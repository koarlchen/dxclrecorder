use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io;
use std::io::BufReader;
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

#[derive(Serialize, Deserialize)]
struct Configuration {
    constrings: Vec<String>,
}

struct ConnectionInformation {
    callsign: String,
    host: String,
    port: u16,
}

fn parse_config(path: &Path) -> Result<Configuration, io::Error> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let config: Configuration = serde_json::from_reader(reader)?;
    Ok(config)
}

fn parse_constring(raw: &str) -> Option<ConnectionInformation> {
    lazy_static! {
        static ref RE_CONSTR: Regex =
            Regex::new(r#"^(?P<user>.+)@(?P<host>.+):(?P<port>\d+)$"#).unwrap();
    }

    if let Some(cap) = RE_CONSTR.captures(raw) {
        let user = cap.name("user").unwrap().as_str();
        let host = cap.name("host").unwrap().as_str();
        let port = cap.name("port").unwrap().as_str().parse::<u16>();

        if let Ok(p) = port {
            return Some(ConnectionInformation {
                callsign: user.into(),
                host: host.into(),
                port: p,
            });
        }
    }

    None
}

fn main() {
    // Read json configuration
    let config =
        parse_config(Path::new("dxclrecorder.json")).expect("Failed to read configuration");

    // Parse connection strings
    let mut coninfo: Vec<ConnectionInformation> = Vec::new();
    for constring in config.constrings.iter() {
        match parse_constring(constring) {
            Some(info) => coninfo.push(info),
            None => {
                eprintln!("Found invalid connection string: '{}'", constring);
                process::exit(1);
            }
        }
    }

    // Communication channel between listeners and receiver
    let (tx, rx) = mpsc::channel();

    // Create two listener
    let mut listeners: Vec<dxcllistener::Listener> = Vec::new();
    for con in coninfo.iter() {
        listeners.push(
            dxcllistener::listen(con.host.clone(), con.port, con.callsign.clone(), tx.clone())
                .unwrap(),
        );
    }

    // Handle incoming spots
    thread::spawn(move || {
        while let Ok(spot) = rx.recv() {
            println!("{}", spot.to_json());
        }
    });

    // Stop signal
    let signal = Arc::new(AtomicBool::new(true));

    // Register ctrl-c handler to stop threads
    let sig = signal.clone();
    ctrlc::set_handler(move || {
        println!("Ctrl-C caught");
        sig.store(false, Ordering::Relaxed);
    })
    .expect("Failed to listen on Ctrl-C");

    while !listeners.is_empty() {
        // Check for application stop request
        if !signal.load(Ordering::Relaxed) {
            for l in listeners.iter_mut() {
                l.request_stop();
            }
            for l in listeners.iter_mut() {
                l.join().unwrap();
            }
            break;
        }

        // Check for unexpectedly stopped listeners and remove them from the list of active listeners
        listeners.retain_mut(|l| {
            if !l.is_running() {
                let res = l.join().unwrap_err();
                println!(
                    "Listener {}@{}:{} stopped unexpectedly ({})",
                    l.callsign, l.host, l.port, res
                );
                return false;
            }
            true
        });

        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}
