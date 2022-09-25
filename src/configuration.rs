use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io;
use std::{io::BufReader, path::Path};

#[derive(Serialize, Deserialize, Clone)]
pub struct Configuration {
    pub connection: Connection,
    pub logging: Logging,
    pub output: Output,
    pub filter: Filter,
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
    pub filename: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Filter {
    pub r#type: Type,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Type {
    pub dx: bool,
    pub wx: bool,
    pub wwv: bool,
    pub wcy: bool,
    pub toall: bool,
    pub tolocal: bool,
}

/// Parse configuration
pub fn parse_config(path: &Path) -> Result<Configuration, io::Error> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let config: Configuration = serde_json::from_reader(reader)?;
    Ok(config)
}
