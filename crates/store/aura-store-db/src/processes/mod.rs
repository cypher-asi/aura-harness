//! First-class processes / automations (Swarm TEE upgrade phase 7).
//!
//! A *process* is a cron-scheduled instruction the agent executes
//! autonomously: `name + cron + prompt (+ optional JSON config)`.
//! Definitions live in the `processes` column family and execution
//! history in the `process_runs` column family of the shared
//! agent-state RocksDB, both behind the same per-value sealing
//! envelope as the secrets vault (see [`crate::seal`] and
//! [`crate::vault`]):
//!
//! * **Sealed mode** (`AURA_STATE_ENCRYPTION=sealed`): process records
//!   (including the prompt and config) and run records are AES-256-GCM
//!   encrypted under the per-agent DEK — never written to disk in
//!   plaintext.
//! * **Plaintext/dev mode**: plain JSON, like the rest of the state.
//!
//! # Trust boundary ("trigger outside, data inside")
//!
//! The process prompt, config, and run history are **in-TEE data** and
//! must never leave the agent VM. The only process-derived data
//! allowed off-VM is the trigger metadata returned by
//! [`ProcessStore::trigger_metadata`]: `(process_id, cron, enabled,
//! next_run_at)` — enough for an external cron service to know *when*
//! to fire a trigger, and nothing about *what* the trigger does.
//!
//! [`ProcessRecord`] has a manual `Debug` impl that redacts the prompt
//! and config so neither can leak through `{:?}` formatting in logs.

//!
//! # Layout
//!
//! * `records`: the persisted and exported types.
//! * `schedule`: cron parsing and input validation.
//! * `definitions`: create / read / update / delete of process
//!   definitions and the trigger export.
//! * `runs`: run history, its key format and pruning.

use crate::seal::SealCipher;
use rocksdb::{DBWithThreadMode, MultiThreaded};
use std::sync::Arc;

mod definitions;
mod records;
mod runs;
mod schedule;

pub use records::{
    NewProcess, ProcessRecord, ProcessRunRecord, ProcessRunStatus, ProcessTriggerMeta,
    ProcessUpdate,
};
pub use schedule::{next_run_after, validate_cron};

/// Maximum length of a process name.
pub const MAX_PROCESS_NAME_LEN: usize = 200;

/// Maximum size of a process prompt in bytes.
pub const MAX_PROCESS_PROMPT_BYTES: usize = 64 * 1024;

/// Run records kept per process; older runs are pruned on insert.
pub const MAX_PROCESS_RUNS_KEPT: usize = 50;

/// Errors produced by [`ProcessStore`].
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    /// The process name is empty or too long.
    #[error("invalid process name: {0}")]
    InvalidName(String),
    /// The cron expression failed to parse.
    #[error("invalid cron expression: {0}")]
    InvalidCron(String),
    /// The prompt is empty or exceeds [`MAX_PROCESS_PROMPT_BYTES`].
    #[error("invalid process prompt: {0}")]
    InvalidPrompt(String),
    /// No process with the given id exists.
    #[error("process not found: {0}")]
    NotFound(String),
    /// No run with the given id exists for the process.
    #[error("process run not found: {0}")]
    RunNotFound(String),
    /// Underlying RocksDB / sealing failure.
    #[error("process store error: {0}")]
    Store(String),
    /// JSON (de)serialization failure.
    #[error("process serialization error: {0}")]
    Serde(String),
}

/// Sealed store for process definitions and run history, backed by the
/// shared agent-state RocksDB. Same construction pattern as
/// [`crate::vault::SecretsVault`]: shared DB handle plus the optional
/// value cipher decided at boot.
pub struct ProcessStore {
    db: Arc<DBWithThreadMode<MultiThreaded>>,
    /// `Some` in sealed mode (records are AES-256-GCM ciphertext at
    /// rest); `None` is plaintext/dev mode.
    cipher: Option<Arc<SealCipher>>,
}

impl ProcessStore {
    /// Create a plaintext-mode store on the given shared DB handle.
    #[must_use]
    pub const fn new(db: Arc<DBWithThreadMode<MultiThreaded>>) -> Self {
        Self { db, cipher: None }
    }

    /// Create a store with optional sealed (encrypted-at-rest) values.
    #[must_use]
    pub const fn with_cipher(
        db: Arc<DBWithThreadMode<MultiThreaded>>,
        cipher: Option<Arc<SealCipher>>,
    ) -> Self {
        Self { db, cipher }
    }

    fn seal_value(&self, plain: Vec<u8>) -> Result<Vec<u8>, ProcessError> {
        match &self.cipher {
            Some(cipher) => cipher
                .seal(&plain)
                .map_err(|e| ProcessError::Store(format!("sealing value: {e}"))),
            None => Ok(plain),
        }
    }

    fn open_value<'a>(&self, bytes: &'a [u8]) -> Result<std::borrow::Cow<'a, [u8]>, ProcessError> {
        match &self.cipher {
            Some(cipher) => cipher
                .open(bytes)
                .map(std::borrow::Cow::Owned)
                .map_err(|e| ProcessError::Store(format!("opening sealed value: {e}"))),
            None => Ok(std::borrow::Cow::Borrowed(bytes)),
        }
    }

    fn processes_cf(&self) -> Result<Arc<rocksdb::BoundColumnFamily<'_>>, ProcessError> {
        self.db
            .cf_handle(crate::cf::PROCESSES)
            .ok_or_else(|| ProcessError::Store("processes column family not found".into()))
    }

    fn runs_cf(&self) -> Result<Arc<rocksdb::BoundColumnFamily<'_>>, ProcessError> {
        self.db
            .cf_handle(crate::cf::PROCESS_RUNS)
            .ok_or_else(|| ProcessError::Store("process_runs column family not found".into()))
    }
}

impl std::fmt::Debug for ProcessStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never expose contents or key material.
        f.debug_struct("ProcessStore")
            .field("sealed", &self.cipher.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests;
