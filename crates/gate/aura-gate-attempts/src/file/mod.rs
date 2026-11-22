//! The log on disk: one JSON object per line, appended to one line at a
//! time and never rewritten.

use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use crate::line::Line;

/// The log's path under the data directory.
pub const FILE: &str = "gate/attempts.jsonl";

/// The log file, appended to one line at a time.
#[derive(Debug)]
pub struct AttemptLog {
    path: PathBuf,
    /// Held while a line is written, so lines of concurrent turns do not
    /// interleave
    writing: Mutex<()>,
}

impl AttemptLog {
    /// The log of the data directory `data_dir`.
    #[must_use]
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(FILE),
            writing: Mutex::new(()),
        }
    }

    /// Where the log is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append `line`.
    ///
    /// # Errors
    ///
    /// When the file or its directory cannot be written.
    pub fn append(&self, line: &Line) -> io::Result<()> {
        let mut text = serde_json::to_string(line)?;
        text.push('\n');
        let _held = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?
            .write_all(text.as_bytes())
    }

    /// The log's lines, oldest first; none when there is no file yet.
    ///
    /// # Errors
    ///
    /// When the file cannot be read.
    pub fn read(&self) -> io::Result<Read> {
        read(&self.path)
    }
}

/// The lines of a log, and how many of its lines could not be read as
/// one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Read {
    /// The lines, oldest first
    pub lines: Vec<Line>,
    /// Lines that are not a line of the log
    pub unreadable: usize,
}

/// The lines of the log at `path`, oldest first; none when there is no
/// file.
///
/// # Errors
///
/// When the file cannot be read.
pub fn read(path: &Path) -> io::Result<Read> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Read::default()),
        Err(e) => return Err(e),
    };
    let mut out = Read::default();
    for text in contents.lines().filter(|text| !text.trim().is_empty()) {
        match serde_json::from_str(text) {
            Ok(line) => out.lines.push(line),
            Err(_) => out.unreadable += 1,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
