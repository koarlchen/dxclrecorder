// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use async_compression::tokio::write::GzipEncoder;
use async_compression::Level;
use chrono::{DateTime, Datelike, Utc};
use std::path::PathBuf;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{self, AsyncWriteExt, BufReader, BufWriter};
use tokio::time::{Duration, Instant};

/// Custom handler to write to file.
/// Supports adding a date to the filename and rotate the file daily.
pub struct FileWriter {
    /// Configured filename
    config_filename: PathBuf,

    /// Currently used filename
    current_filename: Option<String>,

    /// Date of last modification
    last_modification: DateTime<Utc>,

    /// Buffered writer for data file
    writer: Option<BufWriter<File>>,

    /// File rotation enabled
    rotate: bool,

    /// File compression enabled
    compress: bool,
}

impl FileWriter {
    /// Create new file writer.
    ///
    /// # Arguments
    ///
    /// * `fname`: Name of file to write to
    /// * `rotate`: True to rotate file daily
    /// * `date`: True to add date to filename. The date will be added directly in front of the extension.
    ///
    /// # Result
    ///
    /// Returns a new instance or propagates an error while initialization.
    pub async fn new(fname: PathBuf, rotate: bool, compress: bool) -> Result<Self, io::Error> {
        let mut new = Self {
            config_filename: fname,
            current_filename: None,
            last_modification: Utc::now(),
            writer: None,
            rotate,
            compress,
        };

        new.rotate(new.last_modification).await?;

        Ok(new)
    }

    /// Rotate file.
    /// Flush and close the previous file (if present) and create a new file.
    ///
    /// # Arguments
    ///
    /// * `date`: Date of the file to create
    ///
    /// # Result
    ///
    /// Nothing or the encountered error.
    async fn rotate(&mut self, date: DateTime<Utc>) -> Result<(), io::Error> {
        // Flush writer with last data
        if self.writer.is_some() {
            self.flush().await?;
            self.writer = None;
        }

        // Compress file (if enabled)
        if self.compress && self.current_filename.is_some() {
            let fname_curr = self.current_filename.as_ref().unwrap().clone();
            let mut fname_out = fname_curr.clone();
            fname_out.push_str(".gz");

            tokio::task::spawn(async move {
                match compress(&fname_curr, &fname_out).await {
                    Ok(dur) => {
                        info!("Compressed data file (took {} msec)", dur.as_millis());
                        if fs::remove_file(fname_curr).await.is_err() {
                            warn!("Failed to remove file after compression");
                        }
                    }
                    Err(err) => warn!("Failed to compress data file ({})", err),
                }
            });
        }

        // Open new file
        let date_str = format!("{:04}{:02}{:02}", date.year(), date.month(), date.day());
        let fname = self
            .config_filename
            .display()
            .to_string()
            .replace("{DATE}", &date_str);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&fname)
            .await?;
        self.writer = Some(BufWriter::with_capacity(1024, file));

        self.current_filename = Some(fname);

        Ok(())
    }

    /// Write new data to file.
    ///
    /// # Arguments
    ///
    /// * `data`: Data to write to file
    ///
    /// # Result
    ///
    /// Nothing or the encountered error.
    pub async fn write(&mut self, data: &str) -> Result<(), io::Error> {
        let now = Utc::now();

        if now.date_naive() > self.last_modification.date_naive() && self.rotate {
            self.rotate(now).await?;
        }

        self.writer
            .as_mut()
            .unwrap()
            .write_all(format!("{}\n", data).as_bytes())
            .await?;
        self.last_modification = now;

        Ok(())
    }

    /// Flush the internal buffer into file.
    ///
    /// # Arguments
    ///
    /// (none)
    ///
    /// # Result
    ///
    /// Nothing or the encountered error.
    pub async fn flush(&mut self) -> Result<(), io::Error> {
        self.writer.as_mut().unwrap().flush().await
    }
}

/// Compress file with gzip.
///
/// # Arguments
///
/// - `fname_in`: Name of file to compress
/// - `fname_out`: Name of file to write compressed data to
///
/// # Result
///
/// Duration of compression or the encountered error.
pub async fn compress(fname_in: &String, fname_out: &String) -> io::Result<Duration> {
    let input_file = File::open(fname_in).await?;
    let mut input = BufReader::new(input_file);

    let output = File::create(fname_out).await?;
    let mut encoder = GzipEncoder::with_quality(output, Level::Best);

    let before = Instant::now();
    io::copy(&mut input, &mut encoder).await?;
    encoder.shutdown().await?;
    let after = Instant::now();

    Ok(after - before)
}
