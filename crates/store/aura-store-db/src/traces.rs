//! Run traces in the agent-state database.
//!
//! The trace recorder writes every run to an on-disk bundle under
//! `AURA_HOME/runs/`; this store keeps the same data next to the record
//! log so an agent's runs survive with its state and can be read back
//! without the bundle. Values are opaque JSON documents owned by
//! `aura-agent-trace` (run metadata, scorecard, steps, rewards, kernel
//! record rows), sealed at rest in sealed mode like the rest of the
//! content-bearing state.
//!
//! Column families:
//!
//! * [`crate::cf::TRACE_RUNS`]: `meta/{run_id}` and `summary/{run_id}`.
//! * [`crate::cf::TRACE_STEPS`]: `{run_id}/{kind}/{n:020}`, where `kind`
//!   is `s` (steps, `n` = step seq), `w` (rewards) or `r` (record rows),
//!   so one prefix scan reads a run's entries of one kind in order.
//! * [`crate::cf::TRACE_INDEX`]: `{owner}/{run_id}` with an empty value,
//!   the runs of each owner (an agent id in hex). Run ids sort by start
//!   time, so the newest run of an owner is the last key of its prefix.
//!   Keys only, so plaintext in both modes.

use crate::seal::SealCipher;
use rocksdb::{DBWithThreadMode, Direction, IteratorMode, MultiThreaded, WriteBatch};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Errors produced by [`TraceStore`].
#[derive(Debug, thiserror::Error)]
pub enum TraceStoreError {
    /// The run id or owner can't be used in a key.
    #[error("invalid trace key: {0}")]
    InvalidKey(String),
    /// Underlying `RocksDB` or sealing failure.
    #[error("trace store error: {0}")]
    Store(String),
}

/// What kind of per-run entry a key holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Step,
    Reward,
    Record,
}

impl EntryKind {
    const fn tag(self) -> char {
        match self {
            Self::Step => 's',
            Self::Reward => 'w',
            Self::Record => 'r',
        }
    }
}

fn check_part(part: &str, what: &str) -> Result<(), TraceStoreError> {
    if part.is_empty() || part.contains('/') {
        return Err(TraceStoreError::InvalidKey(format!(
            "{what} must be non-empty and contain no '/': {part:?}"
        )));
    }
    Ok(())
}

fn entry_prefix(run_id: &str, kind: EntryKind) -> Vec<u8> {
    format!("{run_id}/{}/", kind.tag()).into_bytes()
}

fn entry_key(run_id: &str, kind: EntryKind, n: u64) -> Vec<u8> {
    format!("{run_id}/{}/{n:020}", kind.tag()).into_bytes()
}

/// Strictly increasing order for rewards and record rows, which carry
/// no sequence of their own. Seeded from wall-clock nanoseconds so it
/// stays ordered across restarts.
fn next_append_seq() -> u64 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
    let mut prev = LAST.load(Ordering::SeqCst);
    loop {
        let next = now.max(prev + 1);
        match LAST.compare_exchange(prev, next, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return next,
            Err(p) => prev = p,
        }
    }
}

/// Run traces in the shared agent-state `RocksDB`.
pub struct TraceStore {
    db: Arc<DBWithThreadMode<MultiThreaded>>,
    /// `Some` in sealed mode.
    cipher: Option<Arc<SealCipher>>,
}

impl std::fmt::Debug for TraceStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceStore")
            .field("sealed", &self.cipher.is_some())
            .finish()
    }
}

impl TraceStore {
    /// Plaintext store on a shared DB handle.
    #[must_use]
    pub const fn new(db: Arc<DBWithThreadMode<MultiThreaded>>) -> Self {
        Self { db, cipher: None }
    }

    /// Store with optional sealed values.
    #[must_use]
    pub const fn with_cipher(
        db: Arc<DBWithThreadMode<MultiThreaded>>,
        cipher: Option<Arc<SealCipher>>,
    ) -> Self {
        Self { db, cipher }
    }

    /// Store sharing `store`'s database and sealing mode.
    #[must_use]
    pub fn for_store(store: &crate::RocksStore) -> Self {
        Self::with_cipher(Arc::clone(store.db_handle()), store.cipher().cloned())
    }

    fn cf(&self, name: &str) -> Result<Arc<rocksdb::BoundColumnFamily<'_>>, TraceStoreError> {
        self.db
            .cf_handle(name)
            .ok_or_else(|| TraceStoreError::Store(format!("{name} column family not found")))
    }

    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, TraceStoreError> {
        match &self.cipher {
            Some(cipher) => cipher
                .seal(plain)
                .map_err(|e| TraceStoreError::Store(format!("sealing value: {e}"))),
            None => Ok(plain.to_vec()),
        }
    }

    fn open(&self, bytes: &[u8]) -> Result<Vec<u8>, TraceStoreError> {
        match &self.cipher {
            Some(cipher) => cipher
                .open(bytes)
                .map_err(|e| TraceStoreError::Store(format!("opening sealed value: {e}"))),
            None => Ok(bytes.to_vec()),
        }
    }

    fn get(&self, cf: &str, key: &[u8]) -> Result<Option<Vec<u8>>, TraceStoreError> {
        let cf = self.cf(cf)?;
        self.db
            .get_cf(&cf, key)
            .map_err(|e| TraceStoreError::Store(e.to_string()))?
            .map(|bytes| self.open(&bytes))
            .transpose()
    }

    fn scan_keys(&self, cf: &str, prefix: &[u8]) -> Result<Vec<Vec<u8>>, TraceStoreError> {
        let cf = self.cf(cf)?;
        let mut keys = Vec::new();
        for item in self
            .db
            .iterator_cf(&cf, IteratorMode::From(prefix, Direction::Forward))
        {
            let (k, _) = item.map_err(|e| TraceStoreError::Store(e.to_string()))?;
            if !k.starts_with(prefix) {
                break;
            }
            keys.push(k.to_vec());
        }
        Ok(keys)
    }

    fn scan_values(&self, cf: &str, prefix: &[u8]) -> Result<Vec<Vec<u8>>, TraceStoreError> {
        let handle = self.cf(cf)?;
        let mut out = Vec::new();
        for item in self
            .db
            .iterator_cf(&handle, IteratorMode::From(prefix, Direction::Forward))
        {
            let (k, v) = item.map_err(|e| TraceStoreError::Store(e.to_string()))?;
            if !k.starts_with(prefix) {
                break;
            }
            out.push(self.open(&v)?);
        }
        Ok(out)
    }

    /// Store a run's metadata, listing it under `owner` when given.
    ///
    /// # Errors
    /// Rejects ids that can't be keys; surfaces store failures.
    pub fn put_meta(
        &self,
        run_id: &str,
        owner: Option<&str>,
        meta: &[u8],
    ) -> Result<(), TraceStoreError> {
        check_part(run_id, "run id")?;
        let mut batch = WriteBatch::default();
        batch.put_cf(
            &self.cf(crate::cf::TRACE_RUNS)?,
            format!("meta/{run_id}"),
            self.seal(meta)?,
        );
        if let Some(owner) = owner {
            check_part(owner, "owner")?;
            batch.put_cf(
                &self.cf(crate::cf::TRACE_INDEX)?,
                format!("{owner}/{run_id}"),
                [],
            );
        }
        self.db
            .write(batch)
            .map_err(|e| TraceStoreError::Store(e.to_string()))
    }

    /// Store a run's scorecard.
    ///
    /// # Errors
    /// Rejects ids that can't be keys; surfaces store failures.
    pub fn put_summary(&self, run_id: &str, summary: &[u8]) -> Result<(), TraceStoreError> {
        check_part(run_id, "run id")?;
        let cf = self.cf(crate::cf::TRACE_RUNS)?;
        self.db
            .put_cf(&cf, format!("summary/{run_id}"), self.seal(summary)?)
            .map_err(|e| TraceStoreError::Store(e.to_string()))
    }

    /// Store steps of a run by their sequence number. Writing a seq
    /// again replaces it.
    ///
    /// # Errors
    /// Rejects ids that can't be keys; surfaces store failures.
    pub fn put_steps<'a>(
        &self,
        run_id: &str,
        steps: impl IntoIterator<Item = (u64, &'a [u8])>,
    ) -> Result<(), TraceStoreError> {
        check_part(run_id, "run id")?;
        let cf = self.cf(crate::cf::TRACE_STEPS)?;
        let mut batch = WriteBatch::default();
        for (seq, step) in steps {
            batch.put_cf(
                &cf,
                entry_key(run_id, EntryKind::Step, seq),
                self.seal(step)?,
            );
        }
        self.db
            .write(batch)
            .map_err(|e| TraceStoreError::Store(e.to_string()))
    }

    fn append(
        &self,
        run_id: &str,
        kind: EntryKind,
        values: &[Vec<u8>],
    ) -> Result<(), TraceStoreError> {
        check_part(run_id, "run id")?;
        let cf = self.cf(crate::cf::TRACE_STEPS)?;
        let mut batch = WriteBatch::default();
        for value in values {
            batch.put_cf(
                &cf,
                entry_key(run_id, kind, next_append_seq()),
                self.seal(value)?,
            );
        }
        self.db
            .write(batch)
            .map_err(|e| TraceStoreError::Store(e.to_string()))
    }

    /// Append rewards to a run.
    ///
    /// # Errors
    /// Rejects ids that can't be keys; surfaces store failures.
    pub fn append_rewards(&self, run_id: &str, rewards: &[Vec<u8>]) -> Result<(), TraceStoreError> {
        self.append(run_id, EntryKind::Reward, rewards)
    }

    /// Append kernel record rows to a run.
    ///
    /// # Errors
    /// Rejects ids that can't be keys; surfaces store failures.
    pub fn append_records(&self, run_id: &str, rows: &[Vec<u8>]) -> Result<(), TraceStoreError> {
        self.append(run_id, EntryKind::Record, rows)
    }

    /// A run's metadata, if stored.
    ///
    /// # Errors
    /// Surfaces store and unsealing failures.
    pub fn meta(&self, run_id: &str) -> Result<Option<Vec<u8>>, TraceStoreError> {
        self.get(crate::cf::TRACE_RUNS, format!("meta/{run_id}").as_bytes())
    }

    /// A run's scorecard, if stored.
    ///
    /// # Errors
    /// Surfaces store and unsealing failures.
    pub fn summary(&self, run_id: &str) -> Result<Option<Vec<u8>>, TraceStoreError> {
        self.get(
            crate::cf::TRACE_RUNS,
            format!("summary/{run_id}").as_bytes(),
        )
    }

    /// A run's steps in sequence order.
    ///
    /// # Errors
    /// Surfaces store and unsealing failures.
    pub fn steps(&self, run_id: &str) -> Result<Vec<Vec<u8>>, TraceStoreError> {
        self.scan_values(
            crate::cf::TRACE_STEPS,
            &entry_prefix(run_id, EntryKind::Step),
        )
    }

    /// A run's rewards in the order they were added.
    ///
    /// # Errors
    /// Surfaces store and unsealing failures.
    pub fn rewards(&self, run_id: &str) -> Result<Vec<Vec<u8>>, TraceStoreError> {
        self.scan_values(
            crate::cf::TRACE_STEPS,
            &entry_prefix(run_id, EntryKind::Reward),
        )
    }

    /// A run's kernel record rows in the order they were added.
    ///
    /// # Errors
    /// Surfaces store and unsealing failures.
    pub fn records(&self, run_id: &str) -> Result<Vec<Vec<u8>>, TraceStoreError> {
        self.scan_values(
            crate::cf::TRACE_STEPS,
            &entry_prefix(run_id, EntryKind::Record),
        )
    }

    /// Run ids listed under `owner`, oldest first.
    ///
    /// # Errors
    /// Surfaces store failures.
    pub fn runs_of(&self, owner: &str) -> Result<Vec<String>, TraceStoreError> {
        let prefix = format!("{owner}/");
        Ok(self
            .scan_keys(crate::cf::TRACE_INDEX, prefix.as_bytes())?
            .into_iter()
            .filter_map(|k| {
                String::from_utf8(k)
                    .ok()
                    .and_then(|k| k.strip_prefix(&prefix).map(str::to_string))
            })
            .collect())
    }

    /// The newest run listed under `owner`.
    ///
    /// # Errors
    /// Surfaces store failures.
    pub fn latest_run_of(&self, owner: &str) -> Result<Option<String>, TraceStoreError> {
        Ok(self.runs_of(owner)?.pop())
    }

    /// Every stored run id, oldest first.
    ///
    /// # Errors
    /// Surfaces store failures.
    pub fn run_ids(&self) -> Result<Vec<String>, TraceStoreError> {
        Ok(self
            .scan_keys(crate::cf::TRACE_RUNS, b"meta/")?
            .into_iter()
            .filter_map(|k| {
                String::from_utf8(k)
                    .ok()
                    .and_then(|k| k.strip_prefix("meta/").map(str::to_string))
            })
            .collect())
    }

    /// Delete a run: metadata, scorecard, entries and index keys.
    ///
    /// # Errors
    /// Surfaces store failures.
    pub fn delete_run(&self, run_id: &str) -> Result<(), TraceStoreError> {
        check_part(run_id, "run id")?;
        let runs = self.cf(crate::cf::TRACE_RUNS)?;
        let steps = self.cf(crate::cf::TRACE_STEPS)?;
        let index = self.cf(crate::cf::TRACE_INDEX)?;
        let mut batch = WriteBatch::default();
        batch.delete_cf(&runs, format!("meta/{run_id}"));
        batch.delete_cf(&runs, format!("summary/{run_id}"));
        for key in self.scan_keys(crate::cf::TRACE_STEPS, format!("{run_id}/").as_bytes())? {
            batch.delete_cf(&steps, key);
        }
        let suffix = format!("/{run_id}");
        for item in self.db.iterator_cf(&index, IteratorMode::Start) {
            let (k, _) = item.map_err(|e| TraceStoreError::Store(e.to_string()))?;
            if k.ends_with(suffix.as_bytes()) {
                batch.delete_cf(&index, k);
            }
        }
        self.db
            .write(batch)
            .map_err(|e| TraceStoreError::Store(e.to_string()))
    }
}

#[cfg(test)]
mod tests;
