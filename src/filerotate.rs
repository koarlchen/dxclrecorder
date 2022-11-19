// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use chrono::{Date, Datelike, Utc};
use std::path::PathBuf;
use tokio::fs::{File, OpenOptions};
use tokio::io::{self, AsyncWriteExt, BufWriter};

/// Custom handler to write to file.
/// Supports adding a date to the filename and rotate the file daily.
pub struct FileWriter {
    filename: PathBuf,
    last_modification: Date<Utc>,
    writer: Option<BufWriter<File>>,
    rotate: bool,
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
    pub async fn new(fname: PathBuf, rotate: bool) -> Result<Self, io::Error> {
        let mut new = Self {
            filename: fname,
            last_modification: Utc::today(),
            writer: None,
            rotate,
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
    async fn rotate(&mut self, date: Date<Utc>) -> Result<(), io::Error> {
        if self.writer.is_some() {
            self.flush().await?;
            self.writer = None;
        }

        let date_str = format!("{:04}{:02}{:02}", date.year(), date.month(), date.day());
        let fname = self
            .filename
            .display()
            .to_string()
            .replace("{DATE}", &date_str);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(fname)
            .await?;
        self.writer = Some(BufWriter::with_capacity(1024, file));

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
        let now = Utc::today();

        if now > self.last_modification && self.rotate {
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
