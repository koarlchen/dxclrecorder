#[macro_use]
extern crate log;

use dxcllistener::Listener;
use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use simplelog::{
    format_description, ColorChoice, CombinedLogger, ConfigBuilder, LevelFilter, TermLogger,
    TerminalMode,
};
use std::fs::File;
use std::io;
use std::io::BufReader;
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

#[derive(Serialize, Deserialize, Clone)]
struct Configuration {
    constrings: Vec<String>,
    reconnect: bool,
    retries: u64,
    backoff: u64,
}

/// Parse configuration
fn parse_config(path: &Path) -> Result<Configuration, io::Error> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let config: Configuration = serde_json::from_reader(reader)?;
    Ok(config)
}

/// Parse connection string from configuration
fn parse_constring(raw: &str) -> Option<Listener> {
    lazy_static! {
        static ref RE_CONSTR: Regex =
            Regex::new(r#"^(?P<user>.+)@(?P<host>.+):(?P<port>\d+)$"#).unwrap();
    }

    if let Some(cap) = RE_CONSTR.captures(raw) {
        let user = cap.name("user").unwrap().as_str();
        let host = cap.name("host").unwrap().as_str();
        let port = cap.name("port").unwrap().as_str().parse::<u16>();

        if let Ok(p) = port {
            return Some(Listener::new(host.into(), p, user.into()));
        }
    }

    None
}

/// Initialize logging
fn init_logging() {
    let log_config = ConfigBuilder::new()
        .set_time_format_custom(format_description!(
            "[day].[month].[year] [hour]:[minute]:[second]"
        ))
        .build();

    CombinedLogger::init(vec![
        TermLogger::new(
            LevelFilter::Info,
            log_config,
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        /*WriteLogger::new(
            LevelFilter::Info,
            log_config,
            File::create(Path::new(&config.log_dir).join(&config.log_file))
                .expect("Failed to create log file"),
        ),*/
    ])
    .unwrap();
}

fn main() {
    init_logging();

    // Read json configuration
    let config =
        parse_config(Path::new("dxclrecorder.json")).expect("Failed to read configuration");

    // Parse connection strings
    let listeners: Arc<Mutex<Vec<Listener>>> = Arc::new(Mutex::new(Vec::new()));
    for constring in config.constrings.iter() {
        match parse_constring(constring) {
            Some(info) => {
                listeners.lock().unwrap().push(info);
            }
            None => {
                error!("Found invalid connection string: {}", constring);
                process::exit(1);
            }
        }
    }

    // Communication channel between listeners and receiver
    let (tx, rx) = mpsc::channel::<dxcllistener::Spot>();

    // Handle incoming spots
    thread::spawn(move || {
        debug!("Receiving thread started");

        //let mut file = File::create("foo.txt").unwrap();
        while let Ok(spot) = rx.recv() {
            //file.write_all(spot.to_json().as_bytes()).unwrap();
            println!("{}", spot.to_json());
        }

        debug!("Receiving thread finished");
    });

    // Stop signal
    let signal = Arc::new(AtomicBool::new(true));

    // Register ctrl-c handler to stop listeners
    let sig = signal.clone();
    ctrlc::set_handler(move || {
        debug!("Ctrl-C caught");
        sig.store(false, Ordering::Relaxed);
    })
    .expect("Failed to listen on Ctrl-C");

    // Start all listeners and remove from list afterwards
    listeners.lock().unwrap().retain_mut(|l| {
        connect_listener(
            Listener::new(l.host.clone(), l.port, l.callsign.clone()),
            listeners.clone(),
            config.clone(),
            tx.clone(),
        );
        false
    });

    // Main process loop
    loop {
        // Check for application stop request
        if !signal.load(Ordering::Relaxed) {
            let mut lis = listeners.lock().unwrap();

            debug!("Request stop of listeners");
            for l in lis.iter_mut() {
                l.request_stop();
            }
            debug!("Join listeners");
            for l in lis.iter_mut() {
                l.join().unwrap();
            }
            break;
        }

        // Check for unexpectedly stopped listeners and remove them from the list of active listeners
        listeners.lock().unwrap().retain_mut(|l| {
            if !l.is_running() {
                let res = l.join().unwrap_err();
                warn!(
                    "Listener {}@{}:{} stopped unexpectedly ({})",
                    l.callsign, l.host, l.port, res
                );

                if config.reconnect {
                    let dead = Listener::new(l.host.clone(), l.port, l.callsign.clone());
                    connect_listener(dead, listeners.clone(), config.clone(), tx.clone());
                }

                return false;
            }
            true
        });

        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    process::exit(0);
}

fn connect_listener(
    mut listener: Listener,
    listeners: Arc<Mutex<Vec<Listener>>>,
    config: Configuration,
    tx: mpsc::Sender<dxcllistener::Spot>,
) {
    thread::spawn(move || {
        let mut recon_ctr = config.retries;
        loop {
            info!("Try to connect listener {}", listener);
            if listener.listen(tx.clone()).is_ok() {
                info!("Listener {} connected", listener);
                listeners.lock().unwrap().push(listener);
                break;
            }

            recon_ctr -= 1;
            if recon_ctr == 0 {
                error!("Failed to connect listener {}", listener);
                break;
            }

            std::thread::sleep(std::time::Duration::from_secs(config.backoff));
        }
    });
}
