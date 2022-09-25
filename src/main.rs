#[macro_use]
extern crate log;

use dxcllistener::{Listener, Spot};
use lazy_static::lazy_static;
use regex::Regex;
use simplelog::{
    format_description, ColorChoice, CombinedLogger, ConfigBuilder, LevelFilter, SharedLogger,
    TermLogger, TerminalMode, WriteLogger,
};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};

mod configuration;

fn main() {
    process::exit(app())
}

/// Application method.
fn app() -> i32 {
    // Read json configuration
    let config = configuration::parse_config(Path::new("dxclrecorder.json"))
        .expect("Failed to read configuration");

    // Initialize logging
    init_logging(&config);

    // Parse connection strings
    let listeners: Arc<Mutex<Vec<Listener>>> = Arc::new(Mutex::new(Vec::new()));
    for constring in config.connection.constrings.iter() {
        match parse_constring(constring) {
            Some(info) => {
                listeners.lock().unwrap().push(info);
            }
            None => {
                error!("Found invalid connection string: {}", constring);
                return 1;
            }
        }
    }

    // Communication channel between listeners and receiver
    let (tx, rx) = mpsc::channel::<dxcllistener::Spot>();

    // Start receiver for incoming spots
    let receiver = match start_receiver(config.clone(), rx) {
        Ok(rcv) => rcv,
        Err(err) => {
            error!("Failed to start receiver ({})", err);
            return 1;
        }
    };

    // Stop signal
    let signal = Arc::new(AtomicBool::new(true));

    // Register ctrl-c handler to stop listeners
    let sig = signal.clone();
    ctrlc::set_handler(move || {
        info!("Requested shutdown");
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

                if config.connection.reconnect {
                    let dead = Listener::new(l.host.clone(), l.port, l.callsign.clone());
                    connect_listener(dead, listeners.clone(), config.clone(), tx.clone());
                }

                return false;
            }
            true
        });

        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    // Drop last sender and join receiver thread
    drop(tx);
    receiver.join().unwrap();

    info!("Shutdown");

    // Exit programm
    0
}

/// Start receiver for incoming spots.
/// The incoming data is processed according to the application configuration.
///
/// ## Arguments
///
/// * `config`: Application configuration
/// * `rx`: Receiver of parsed spots
///
/// ## Result
///
/// * `Ok(JoinHandle<()>)`: Returning the thread handle in case the receiver started successfully.
/// * `Err(std::io::Error)`: Returning the error in case the initialization of the reciever failed.
fn start_receiver(
    config: configuration::Configuration,
    rx: Receiver<Spot>,
) -> Result<JoinHandle<()>, std::io::Error> {
    let mut write: Option<BufWriter<File>> = None;
    if config.output.file {
        write = Some(BufWriter::with_capacity(
            1024,
            File::create(Path::new(&config.output.filename))?,
        ));
    }

    // Handle incoming spots
    let thd = thread::Builder::new()
        .name("receiver".into())
        .spawn(move || {
            while let Ok(spot) = rx.recv() {
                if match_filter(&spot, &config) {
                    let entry = spot.to_json();
                    if config.output.console {
                        println!("{}", entry);
                    }
                    if config.output.file {
                        writeln!(write.as_mut().unwrap(), "{}", entry)
                            .expect("Failed to write data to file");
                    }
                }
            }

            warn!("Lost connection to senders, stop waiting for received spots");
        })
        .unwrap();

    Ok(thd)
}

/// Match spot against filter rules.
///
/// ## Arguments
///
/// * `spot`: Spot
/// * `config`: Application Configuration
///
/// ## Result
///
/// True if the spot matches the filter, false if at least one filter critera does not match.
fn match_filter(spot: &dxcllistener::Spot, config: &configuration::Configuration) -> bool {
    match spot {
        dxcllistener::Spot::DX(_) if config.filter.r#type.dx => true,
        dxcllistener::Spot::WX(_) if config.filter.r#type.wx => true,
        dxcllistener::Spot::WWV(_) if config.filter.r#type.wwv => true,
        dxcllistener::Spot::WCY(_) if config.filter.r#type.wcy => true,
        dxcllistener::Spot::ToAll(_) if config.filter.r#type.toall => true,
        dxcllistener::Spot::ToLocal(_) if config.filter.r#type.tolocal => true,
        _ => false,
    }
}

/// (Re-)Connect listener to remote server.
///
/// ## Arguments
/// * `listener`: Listener to reconnect to server
/// * `listeners`: List of active listeners
/// * `config`: Application configuration
/// * `tx`: Sender channel for incoming spots
fn connect_listener(
    mut listener: Listener,
    listeners: Arc<Mutex<Vec<Listener>>>,
    config: configuration::Configuration,
    tx: mpsc::Sender<dxcllistener::Spot>,
) {
    thread::Builder::new()
        .name(format!("connect {}", listener))
        .spawn(move || {
            let mut recon_ctr = config.connection.retries;
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

                std::thread::sleep(std::time::Duration::from_secs(config.connection.backoff));
            }
        })
        .unwrap();
}

/// Parse connection string from configuration.
///
/// ## Arguments
/// * `raw`: Raw connection string in the format call@host:port
///
/// ## Result
/// If the format of the connection string is valid a new `Listener` is returned.
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

/// Initialize logging setup.
///
/// ## Arguments
/// * `config`: Application configuration
fn init_logging(config: &configuration::Configuration) {
    let log_config = ConfigBuilder::new()
        .set_time_format_custom(format_description!(
            "[day].[month].[year] [hour]:[minute]:[second]"
        ))
        .build();

    let mut loggers: Vec<Box<dyn SharedLogger>> = vec![];

    if config.logging.console {
        loggers.push(TermLogger::new(
            LevelFilter::Info,
            log_config.clone(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ))
    }

    if config.logging.file {
        loggers.push(WriteLogger::new(
            LevelFilter::Info,
            log_config,
            File::create(Path::new(&config.logging.filepath).join("dxclrecorder.log"))
                .expect("Failed to create log file"),
        ))
    }

    CombinedLogger::init(loggers).unwrap();
}
