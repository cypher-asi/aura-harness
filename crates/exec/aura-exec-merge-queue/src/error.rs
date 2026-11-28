//! Errors of the merge queue.

use std::path::Path;
use thiserror::Error;

/// Why a queue operation failed.
#[derive(Debug, Error)]
pub enum QueueError {
    /// A file or directory of the queue couldn't be read or written.
    #[error("{path}: {source}")]
    Io {
        /// What was being read or written
        path: String,
        /// The underlying error
        source: std::io::Error,
    },
    /// The queue couldn't be encoded or decoded.
    #[error("merge queue file: {0}")]
    Corrupt(String),
    /// The queue's config couldn't be read.
    #[error("merge queue config {path}: {reason}")]
    Config {
        /// The config file
        path: String,
        /// What is wrong with it
        reason: String,
    },
    /// A `git` command failed or isn't allowed.
    #[error("git {command}: {reason}")]
    Git {
        /// The command, without its arguments' contents beyond the subcommand
        command: String,
        /// What went wrong (stderr, exit status or timeout)
        reason: String,
    },
    /// The folder is not in a git repository.
    #[error("{0} is not in a git repository")]
    NotARepo(String),
}

impl QueueError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }
}
