// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

#[macro_use]
extern crate log;

use clap::Parser;
use dxcllistener::Listener;
use simplelog::{
    format_description, ColorChoice, CombinedLogger, ConfigBuilder, LevelFilter, SharedLogger,
    TermLogger, TerminalMode, WriteLogger,
};
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio::io;
use tokio::signal;
use tokio::sync::{broadcast, mpsc};
use tokio::task;
use tokio::time;

mod configuration;
mod filerotate;

use filerotate::FileWriter;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct CliArgs {
    /// Sets the configuration file to use
    #[arg(short, long, default_value = "dxclrecorder.json")]
    config: String,

    /// Enable verbose log output
    #[arg(short, long, default_value = "false")]
    verbose: bool,
}

/// Errors while recording
#[derive(Error, Debug)]
pub enum RecordError {
    #[error("Invalid connection string")]
    InvalidConnectionString,

    #[error("Failed to create/write data file")]
    DataFileError(#[from] io::Error),

    #[error("Invalid configuration")]
    InvalidConfiguration(String),
}

/// Application method.
#[tokio::main]
async fn main() -> Result<(), RecordError> {
    // Parse commandline arguments
    let args = CliArgs::parse();

    // Read configuration from file
    let config = configuration::parse_config(Path::new(&args.config));
    if let Err(e) = config {
        return Err(RecordError::InvalidConfiguration(format!(
            "Failed to read configuration: {}",
            e
        )));
    }
    let config = config.unwrap();

    // Initialize logging
    init_logging(&config, args.verbose || config.logging.verbose)?;

    info!("Startup");

    // Check number of connection strings
    let num_configured_listeners = config.connection.servers.len() as u32;
    if num_configured_listeners == 0 {
        return Err(RecordError::InvalidConfiguration(
            "No connection strings found".into(),
        ));
    }

    // Communication channel between listeners and receiver to forward received lines
    let (tx, rx) = mpsc::unbounded_channel::<String>();

    // Start receiver for incoming spots
    let receiver_running = Arc::new(AtomicBool::new(true));
    let receiver = match start_receiver(config.clone(), rx, receiver_running.clone()).await {
        Ok(rcv) => rcv,
        Err(err) => {
            error!("Failed to start receiver ({})", err);
            return Err(RecordError::DataFileError(err));
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

    // List of all active listeners
    let listeners: Arc<Mutex<VecDeque<Listener>>> = Arc::new(Mutex::new(VecDeque::new()));

    // Reflects the number of present listeners.
    // Counter will be incremented before first connection attempt for each listener.
    // If the listener could not be connected to the server, even after several retries,
    // it will be removed and so the counter will be decremented.
    let active_listeners: Arc<AtomicU32> = Arc::new(AtomicU32::new(num_configured_listeners));

    // Start all listeners.
    // Each listener will be added to the list of active listeners after its successful connect attempt.
    for server in config.connection.servers.iter() {
        connect_listener(
            Listener::new(
                server.hostname.clone(),
                server.port,
                server.callsign.clone(),
            ),
            listeners.clone(),
            active_listeners.clone(),
            config.clone(),
            tx.clone(),
            shtdwn_tx.subscribe(),
        )
    }

    // Main process loop
    while active_listeners.load(Ordering::Relaxed) == num_configured_listeners {
        // Check if the receiver is still running
        if !receiver_running.load(Ordering::Relaxed) {
            error!("Receiver stopped running");
            break;
        }

        // Check if listeners unexpectedly stopped running
        let mut dead_listeners: Vec<Listener> = Vec::new();
        {
            let mut listeners = listeners.lock().unwrap();
            // TODO Use `Vec::extract_if` if stabilized (currently nightly)
            while let Some(pos) = listeners.iter().position(|x| !x.is_running()) {
                dead_listeners.push(listeners.remove(pos).unwrap());
            }
        }

        // Handle possible found dead listeners
        while let Some(mut dead) = dead_listeners.pop() {
            let res = dead.join().await.unwrap_err();

            warn!(
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
    // The special construct to get a value from the vector is required since otherwise a MutexGuard is held across an await point
    for mut car in std::iter::from_fn(|| listeners.lock().unwrap().pop_front()) {
        car.join().await.unwrap();
    }

    // Drop last sender and join receiver thread
    drop(tx);

    receiver
        .await
        .unwrap()
        .map_err(RecordError::DataFileError)?;

    info!("Shutdown");

    Ok(())
}

/// Start receiver for incoming spots.
/// The incoming data is processed according to the application configuration.
///
/// # Arguments
///
/// * `config`: Application configuration
/// * `rx`: Receiver of parsed spots
/// * `rx_running`: State of receiver task
///
/// # Result
///
/// Returns the join handle if the receiver task was spawnd successfully.
/// Otherwise the occurred error is returned.
async fn start_receiver(
    config: configuration::Configuration,
    rx: mpsc::UnboundedReceiver<String>,
    rx_running: Arc<AtomicBool>,
) -> Result<task::JoinHandle<Result<(), std::io::Error>>, std::io::Error> {
    // Create file writer if required
    let mut writer: Option<FileWriter> = None;
    if config.output.file.enabled {
        writer = Some(
            FileWriter::new(
                Path::new(&config.output.file.filename).to_path_buf(),
                config.output.file.rotate,
                config.output.file.compress,
            )
            .await?,
        );
    }

    // Spawn task to handle received data from server
    let tsk = task::spawn(async move {
        let res = receiver(config, rx, writer).await;
        rx_running.store(false, Ordering::Relaxed);
        res
    });

    Ok(tsk)
}

/// Receiver for spots
///
/// # Arguments
///
/// * `config`: Application configuration
/// * `rx`: Receiver of parsed spots
/// * `writer`: Filer writer
///
/// # Result
///
/// Returns unit type if stop was requested.
/// In case of any error the error is returned.
async fn receiver(
    config: configuration::Configuration,
    mut rx: mpsc::UnboundedReceiver<String>,
    mut writer: Option<FileWriter>,
) -> Result<(), io::Error> {
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
                    writer.as_mut().unwrap().write(&entry).await?;
                }
            }
        } else {
            debug!("Failed to parse spot: '{}'", line);
        }
    }

    // Flush file before quitting
    if config.output.file.enabled {
        writer.as_mut().unwrap().flush().await?;
    }

    Ok(())
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
            if config.filter.band.is_empty() {
                true
            } else if let Ok(band) = hambands::search::get_band_for_frequency(dx.freq) {
                config.filter.band.contains(&band.name.into())
            } else {
                debug!("Failed to get band for freq {}", spot.to_json());
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
    active_listeners: Arc<AtomicU32>,
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
                    match res {
                        Ok(_) => {
                            info!("Listener {} connected", listener);
                            listeners.lock().unwrap().push_back(listener);
                            break;
                        }
                        Err(err) => {
                            warn!("Attempt to connect failed for {} ({})", listener, err);
                        }
                    }
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

/// Initialize logging setup.
///
/// # Arguments
///
/// * `config`: Application configuration
/// * `verbose`: Enable verbose log output
fn init_logging(config: &configuration::Configuration, verbose: bool) -> Result<(), RecordError> {
    let log_config = ConfigBuilder::new()
        .set_time_format_custom(format_description!(
            "[day].[month].[year] [hour]:[minute]:[second].[subsecond digits:3]"
        ))
        .build();

    let mut loggers: Vec<Box<dyn SharedLogger>> = vec![];

    let level = if verbose {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };

    if config.logging.console {
        loggers.push(TermLogger::new(
            level,
            log_config.clone(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ))
    }

    if config.logging.file.enabled {
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(Path::new(&config.logging.file.filename))
            .map_err(|err| RecordError::InvalidConfiguration(err.to_string()))?;

        loggers.push(WriteLogger::new(level, log_config, file))
    }

    CombinedLogger::init(loggers).unwrap();

    Ok(())
}
