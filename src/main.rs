// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

#[macro_use]
extern crate log;

use dxcllistener::Listener;
use lazy_static::lazy_static;
use regex::Regex;
use simplelog::{
    format_description, ColorChoice, CombinedLogger, ConfigBuilder, LevelFilter, SharedLogger,
    TermLogger, TerminalMode, WriteLogger,
};
use std::collections::VecDeque;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use tokio::signal;
use tokio::sync::{broadcast, mpsc};
use tokio::task;
use tokio::time;

mod configuration;
mod filerotate;

use filerotate::FileWriter;

/// Application method.
#[tokio::main]
async fn main() {
    // Read json configuration
    let config = configuration::parse_config(Path::new("dxclrecorder.json"))
        .expect("Failed to read configuration");

    // Initialize logging
    init_logging(&config);

    // Parse connection strings
    let listeners: Arc<Mutex<VecDeque<Listener>>> = Arc::new(Mutex::new(VecDeque::new()));
    for constring in config.connection.constrings.iter() {
        match parse_constring(constring) {
            Some(info) => {
                listeners.lock().unwrap().push_back(info);
            }
            None => {
                error!("Found invalid connection string: {}", constring);
                return;
            }
        }
    }

    // Communication channel between listeners and receiver to forward received lines
    let (tx, rx) = mpsc::unbounded_channel::<String>();

    // Start receiver for incoming spots
    let receiver = match start_receiver(config.clone(), rx).await {
        Ok(rcv) => rcv,
        Err(err) => {
            error!("Failed to start receiver ({})", err);
            return;
        }
    };

    // Shutdown signal shared across all tasks
    // In case the shutdown was requested, an empty message is sent to all receivers.
    // Since this is for single-usage-only a buffer size of 1 for the channel is sufficient.
    let (shtdwn_tx, mut shtdwn_rx) = broadcast::channel::<()>(1);

    // Register ctrl-c handler to stop tasks
    // If the signal was caught, an empty message is sent through the shutdown channel to inform all tasks about the shutdown request.
    let tmp_tx = shtdwn_tx.clone();
    task::spawn(async move {
        signal::ctrl_c().await.expect("Failed to listen for ctrl-c");
        tmp_tx.send(()).unwrap();
    });

    // Refelects the number of present listeners.
    // Counter will be incremented before first connection attempt for each listener.
    // If the listener could not be connected to the server, even after several retries,
    // it will be removed and so the counter will be decremented.
    let active_listeners: Arc<AtomicI32> = Arc::new(AtomicI32::new(0));

    // Start all listeners and remove from them from the list afterwards
    // Each listener will be added again after its successful connect attempt.
    listeners.lock().unwrap().retain_mut(|l| {
        active_listeners.fetch_add(1, Ordering::Relaxed);
        connect_listener(
            Listener::new(l.host.clone(), l.port, l.callsign.clone()),
            listeners.clone(),
            active_listeners.clone(),
            config.clone(),
            tx.clone(),
            shtdwn_tx.subscribe(),
        );
        false
    });

    // Main process loop
    while active_listeners.load(Ordering::Relaxed) > 0 {
        // Check if a listener unexpectedly stopped running
        let mut dead_listener: Option<Listener> = None;
        let mut lis_guard = listeners.lock().unwrap();
        if let Some(pos) = lis_guard.iter_mut().position(|x| !x.is_running()) {
            dead_listener = lis_guard.remove(pos);
        }
        // NOTE: clippy does not recognize explicit drop of mutex guard (causes clippy::await_holding_lock)
        // See also here: https://github.com/rust-lang/rust-clippy/issues/6446
        drop(lis_guard);

        // Handle a possible found dead listener
        if let Some(mut dead) = dead_listener {
            let res = dead.join().await.unwrap_err();

            println!(
                "Listener {}@{}:{} stopped unexpectedly ({})",
                dead.callsign, dead.host, dead.port, res
            );

            if config.connection.reconnect {
                connect_listener(
                    dead,
                    listeners.clone(),
                    active_listeners.clone(),
                    config.clone(),
                    tx.clone(),
                    shtdwn_tx.subscribe(),
                )
            }
        }

        // Wait before check of stopped listeners.
        // In case of a shutdown request break loop to shutdown application
        tokio::select! {
            _ = time::sleep(time::Duration::from_millis(250)) => (),
            _ = shtdwn_rx.recv() => {
                break;
            }
        }
    }

    // Request stop of all listeners
    for l in listeners.lock().unwrap().iter_mut() {
        l.request_stop().unwrap();
    }

    // Join all stopped listeners
    while let Some(mut lis) = listeners.lock().unwrap().pop_front() {
        // TODO: clippy warns about a present mutex guard from the line above in combination with calling await.
        // The mutex guard should already be dropped when working with the removed element (remove happens by pop_front)
        lis.join().await.unwrap();
    }

    // Drop last sender and join receiver thread
    drop(tx);
    receiver.await.unwrap();

    info!("Shutdown");
}

/// Start receiver for incoming spots.
/// The incoming data is processed according to the application configuration.
///
/// # Arguments
///
/// * `config`: Application configuration
/// * `rx`: Receiver of parsed spots
///
/// # Result
///
/// * `Ok(JoinHandle<()>)`: Returning the thread handle in case the receiver started successfully.
/// * `Err(std::io::Error)`: Returning the error in case the initialization of the reciever failed.
async fn start_receiver(
    config: configuration::Configuration,
    mut rx: mpsc::UnboundedReceiver<String>,
) -> Result<task::JoinHandle<()>, std::io::Error> {
    // Create file writer if required
    let mut writer: Option<FileWriter> = None;
    if config.output.file.enabled {
        writer = Some(
            FileWriter::new(
                Path::new(&config.output.file.filename).to_path_buf(),
                false,
                true,
            )
            .await?,
        );
    }

    // Spawn task to handle received data from server
    let tsk = task::spawn(async move {
        // Wait for new data
        while let Some(line) = rx.recv().await {
            // Try to parse spot
            if let Ok(spot) = dxclparser::parse(&line) {
                // Check if the parsed spot does not match to any of the filters
                if match_filter(&spot, &config) {
                    let entry = spot.to_json();

                    // Write spot to console
                    if config.output.console {
                        println!("{}", entry);
                    }

                    // Write spot to file
                    if config.output.file.enabled {
                        writer
                            .as_mut()
                            .unwrap()
                            .write(&entry)
                            .await
                            .expect("Failed to write data to file");
                    }
                }
            }
        }

        // Flush file before quitting
        if config.output.file.enabled {
            writer
                .as_mut()
                .unwrap()
                .flush()
                .await
                .expect("Failed to flush writer");
        }
    });

    Ok(tsk)
}

/// Match spot against filter rules.
///
/// # Arguments
///
/// * `spot`: Spot
/// * `config`: Application Configuration
///
/// # Result
///
/// True if the spot matches the filter, false if at least one filter critera does not match.
fn match_filter(spot: &dxclparser::Spot, config: &configuration::Configuration) -> bool {
    // Filter for type
    let r#type = match spot {
        dxclparser::Spot::DX(_) if config.filter.r#type.dx => true,
        dxclparser::Spot::WX(_) if config.filter.r#type.wx => true,
        dxclparser::Spot::WWV(_) if config.filter.r#type.wwv => true,
        dxclparser::Spot::WCY(_) if config.filter.r#type.wcy => true,
        dxclparser::Spot::ToAll(_) if config.filter.r#type.toall => true,
        dxclparser::Spot::ToLocal(_) if config.filter.r#type.tolocal => true,
        _ => false,
    };

    // Filter for band
    let band = match spot {
        dxclparser::Spot::DX(dx) => {
            if let Ok(band) = hambands::search::get_band_for_frequency(dx.freq) {
                config.filter.band.contains(&band.name.into())
            } else {
                error!("Failed to get band for freq {}", dx.freq);
                false
            }
        }
        _ => true,
    };

    r#type && band
}

/// (Re-)Connect listener to remote server.
///
/// # Arguments
///
/// * `listener`: Listener to reconnect to server
/// * `listeners`: List of active listeners
/// * `config`: Application configuration
/// * `tx`: Sender channel for incoming spots
/// * `signal`: Signal for application shutdown request
fn connect_listener(
    mut listener: Listener,
    listeners: Arc<Mutex<VecDeque<Listener>>>,
    active_listeners: Arc<AtomicI32>,
    config: configuration::Configuration,
    tx: mpsc::UnboundedSender<String>,
    mut shtdwn: broadcast::Receiver<()>,
) {
    // Spawn new task to connect listener to server
    task::spawn(async move {
        let mut recon_ctr = config.connection.retries;

        loop {
            let sleep_instant = time::Instant::now()
                .checked_add(time::Duration::from_secs(config.connection.backoff))
                .unwrap();

            info!("Try to connect listener {}", listener);
            tokio::select! {
                _ = shtdwn.recv() => {
                    break;
                },
                res = listener.listen(tx.clone(), time::Duration::from_secs(1)) => {
                    if res.is_ok() {
                        info!("Listener {} connected", listener);
                        listeners.lock().unwrap().push_back(listener);
                        break;
                    }
                    info!("Attempt to connect failed for {}", listener);
                }
            }

            recon_ctr -= 1;
            if recon_ctr == 0 {
                error!("Failed to connect listener {}", listener);
                active_listeners.fetch_sub(1, Ordering::Relaxed);
                break;
            }

            tokio::select! {
                _ = shtdwn.recv() => {
                    break;
                },
                _ = time::sleep_until(sleep_instant) => {}
            }
        }
    });
}

/// Parse connection string from configuration.
///
/// # Arguments
///
/// * `raw`: Raw connection string in the format call@host:port
///
/// # Result
///
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
/// # Arguments
///
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
            LevelFilter::Debug,
            log_config.clone(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ))
    }

    if config.logging.file.enabled {
        loggers.push(WriteLogger::new(
            LevelFilter::Info,
            log_config,
            File::create(Path::new(&config.logging.file.filename))
                .expect("Failed to create log file"),
        ))
    }

    CombinedLogger::init(loggers).unwrap();
}
