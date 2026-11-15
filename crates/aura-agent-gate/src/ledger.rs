//! Append-only log of gate attempts (`AURA_HOME/gate/ledger.jsonl`).
//!
//! One line per worker attempt: the bucket, the tier and why, the
//! propensity of that choice, whether it passed and what it cost. It
//! is a derived index for fitting and reports; the kernel record of the
//! `run_blocks` call and of each child agent stays the source of truth.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::block::{Attempt, BlockResult, BlockStatus, VerifyLevel};
use crate::tier::Tier;

/// One attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// When the attempt ended.
    pub at_ms: u64,
    /// Policy version that routed it.
    pub policy: String,
    /// Recorded run (eval or trace) it belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Block title.
    pub block: String,
    /// Statistics bucket.
    pub bucket: String,
    /// Attempt number within the block.
    pub attempt: u32,
    /// Tier that ran.
    pub tier: Tier,
    /// Model that ran.
    pub model: String,
    /// Why this tier.
    pub reason: String,
    /// Chosen by exploration.
    #[serde(default)]
    pub explored: bool,
    /// Probability of the choice under the policy.
    pub propensity: f64,
    /// Whether the attempt passed its checks.
    pub passed: bool,
    /// Level reached.
    pub level: VerifyLevel,
    /// Level required.
    pub required: VerifyLevel,
    /// List price, USD.
    pub cost_usd: f64,
    /// Prompt tokens.
    pub input_tokens: u64,
    /// Generated tokens.
    pub output_tokens: u64,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    /// Whether the block as a whole passed.
    pub block_passed: bool,
}

impl LedgerEntry {
    /// Entries for every attempt of `block`.
    #[must_use]
    pub fn for_block(
        block: &BlockResult,
        policy: &str,
        run_id: Option<&str>,
        at_ms: u64,
    ) -> Vec<Self> {
        block
            .attempts
            .iter()
            .map(|a: &Attempt| Self {
                at_ms,
                policy: policy.to_string(),
                run_id: run_id.map(str::to_string),
                block: block.title.clone(),
                bucket: block.bucket.clone(),
                attempt: a.n,
                tier: a.tier,
                model: a.model.clone(),
                reason: a.reason.clone(),
                explored: a.explored,
                propensity: a.propensity,
                passed: a.passed,
                level: a.level,
                required: block.required,
                cost_usd: a.cost_usd,
                input_tokens: a.input_tokens,
                output_tokens: a.output_tokens,
                failure: a.failure.clone(),
                block_passed: block.status == BlockStatus::Passed,
            })
            .collect()
    }
}

/// Appends to one ledger file.
#[derive(Debug)]
pub struct Ledger {
    path: PathBuf,
    lock: Mutex<()>,
}

impl Ledger {
    /// Ledger at `path`; the file is created on first append.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    /// File path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append `entries`.
    ///
    /// # Errors
    /// Fails when the file can't be opened or written.
    pub fn append(&self, entries: &[LedgerEntry]) -> std::io::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let mut buf = Vec::new();
        for entry in entries {
            serde_json::to_writer(&mut buf, entry)?;
            buf.push(b'\n');
        }
        file.write_all(&buf)
    }

    /// Read every entry; unparseable lines are skipped. A missing file
    /// reads as empty.
    ///
    /// # Errors
    /// Fails when the file exists but can't be read.
    pub fn read(path: &Path) -> std::io::Result<Vec<LedgerEntry>> {
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for line in std::io::BufReader::new(file).lines() {
            let line = line?;
            if let Ok(entry) = serde_json::from_str(&line) {
                out.push(entry);
            }
        }
        Ok(out)
    }
}
