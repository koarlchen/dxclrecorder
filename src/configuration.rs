use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io;
use std::{io::BufReader, path::Path};

#[derive(Serialize, Deserialize, Clone)]
pub struct Configuration {
    pub connection: Connection,
    pub logging: Logging,
    pub output: Output,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Connection {
    pub constrings: Vec<String>,
    pub reconnect: bool,
    pub retries: u64,
    pub backoff: u64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Output {
    pub console: bool,
    pub file: bool,
    pub filename: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Logging {
    pub console: bool,
    pub file: bool,
    pub filepath: String,
}

/// Parse configuration
pub fn parse_config(path: &Path) -> Result<Configuration, io::Error> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let config: Configuration = serde_json::from_reader(reader)?;
    Ok(config)
}
