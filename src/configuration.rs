// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

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
    #[serde(rename = "file")]
    pub file: OutputFile,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct OutputFile {
    pub enabled: bool,
    pub date: bool,
    pub rotate: bool,
    pub filename: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Logging {
    pub console: bool,
    #[serde(rename = "file")]
    pub file: LoggingFile,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct LoggingFile {
    pub enabled: bool,
    pub filename: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Filter {
    pub r#type: Type,
    pub band: Vec<String>,
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
///
/// # Arguments
///
/// * `path`: Path to configuration to read
///
/// # Result
///
/// Returns the parsed configuration or propagates the occurred error.
pub fn parse_config(path: &Path) -> Result<Configuration, io::Error> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let config: Configuration = serde_json::from_reader(reader)?;
    Ok(config)
}
