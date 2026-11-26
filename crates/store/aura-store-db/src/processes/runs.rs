//! Run history: run records, their key format, and pruning to
//! [`MAX_PROCESS_RUNS_KEPT`].

use super::schedule::next_run_after;
use super::{
    ProcessError, ProcessRunRecord, ProcessRunStatus, ProcessStore, MAX_PROCESS_RUNS_KEPT,
};
use chrono::Utc;
use rocksdb::IteratorMode;

/// Key for a run record: `{process_id}/{seq:020}/{run_id}`.
///
/// RocksDB's lexicographic key order then sorts a process's runs
/// oldest-first under the `{process_id}/` prefix, which makes both
/// "newest N" listing and oldest-first pruning a single prefix scan.
/// `seq` is the store-assigned strictly-increasing sequence (see
/// [`next_run_seq`]) so two runs started in the same millisecond still
/// order deterministically by start order.
fn run_key(process_id: &str, seq: u64, run_id: &str) -> Vec<u8> {
    format!("{process_id}/{seq:020}/{run_id}").into_bytes()
}

/// Strictly-increasing run sequence, seeded from wall-clock millis so
/// it stays roughly time-ordered across process restarts.
fn next_run_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    #[allow(clippy::cast_sign_loss)]
    let now = Utc::now().timestamp_millis().max(0) as u64;
    let mut prev = LAST.load(Ordering::SeqCst);
    loop {
        let next = now.max(prev + 1);
        match LAST.compare_exchange(prev, next, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return next,
            Err(p) => prev = p,
        }
    }
}

fn run_prefix(process_id: &str) -> Vec<u8> {
    format!("{process_id}/").into_bytes()
}

impl ProcessStore {
    /// Record the start of a run: creates a `Running` run record,
    /// stamps `last_run_at = now`, recomputes `next_run_at`, and prunes
    /// history beyond [`MAX_PROCESS_RUNS_KEPT`].
    ///
    /// # Errors
    /// [`ProcessError::NotFound`] when the process id is unknown.
    pub fn start_run(&self, process_id: &str) -> Result<ProcessRunRecord, ProcessError> {
        let mut record = self
            .get(process_id)?
            .ok_or_else(|| ProcessError::NotFound(process_id.to_string()))?;

        let now = Utc::now();
        let run = ProcessRunRecord {
            run_id: uuid::Uuid::new_v4().to_string(),
            process_id: process_id.to_string(),
            seq: next_run_seq(),
            started_at: now,
            finished_at: None,
            status: ProcessRunStatus::Running,
            summary: None,
            error: None,
        };
        self.put_run(&run)?;

        record.last_run_at = Some(now);
        record.updated_at = now;
        record.next_run_at = next_run_after(&record.cron, now)?;
        self.put_record(&record)?;

        self.prune_runs(process_id)?;
        Ok(run)
    }

    /// Mark a run finished with the given terminal status.
    ///
    /// # Errors
    /// [`ProcessError::RunNotFound`] when no matching run exists (it
    /// may have been pruned).
    pub fn finish_run(
        &self,
        process_id: &str,
        run_id: &str,
        status: ProcessRunStatus,
        summary: Option<String>,
        error: Option<String>,
    ) -> Result<ProcessRunRecord, ProcessError> {
        let mut run = self
            .list_runs(process_id)?
            .into_iter()
            .find(|r| r.run_id == run_id)
            .ok_or_else(|| ProcessError::RunNotFound(run_id.to_string()))?;
        run.finished_at = Some(Utc::now());
        run.status = status;
        run.summary = summary;
        run.error = error;
        self.put_run(&run)?;
        Ok(run)
    }

    /// Run history for a process, newest first. Bounded by
    /// [`MAX_PROCESS_RUNS_KEPT`] via insert-time pruning.
    ///
    /// # Errors
    /// Surfaces store / unsealing / decoding failures.
    pub fn list_runs(&self, process_id: &str) -> Result<Vec<ProcessRunRecord>, ProcessError> {
        let cf = self.runs_cf()?;
        let prefix = run_prefix(process_id);
        let mut out = Vec::new();
        for item in self.db.iterator_cf(
            &cf,
            IteratorMode::From(&prefix, rocksdb::Direction::Forward),
        ) {
            let (k, v) = item.map_err(|e| ProcessError::Store(e.to_string()))?;
            if !k.starts_with(&prefix) {
                break;
            }
            let bytes = self.open_value(&v)?;
            let run: ProcessRunRecord =
                serde_json::from_slice(&bytes).map_err(|e| ProcessError::Serde(e.to_string()))?;
            out.push(run);
        }
        out.reverse(); // key order is oldest-first
        Ok(out)
    }

    fn put_run(&self, run: &ProcessRunRecord) -> Result<(), ProcessError> {
        let plain = serde_json::to_vec(run).map_err(|e| ProcessError::Serde(e.to_string()))?;
        let sealed = self.seal_value(plain)?;
        let cf = self.runs_cf()?;
        self.db
            .put_cf(&cf, run_key(&run.process_id, run.seq, &run.run_id), sealed)
            .map_err(|e| ProcessError::Store(e.to_string()))
    }

    /// All run keys for a process, oldest first.
    pub(super) fn run_keys(&self, process_id: &str) -> Result<Vec<Vec<u8>>, ProcessError> {
        let cf = self.runs_cf()?;
        let prefix = run_prefix(process_id);
        let mut keys = Vec::new();
        for item in self.db.iterator_cf(
            &cf,
            IteratorMode::From(&prefix, rocksdb::Direction::Forward),
        ) {
            let (k, _) = item.map_err(|e| ProcessError::Store(e.to_string()))?;
            if !k.starts_with(&prefix) {
                break;
            }
            keys.push(k.to_vec());
        }
        Ok(keys)
    }

    /// Delete oldest runs beyond [`MAX_PROCESS_RUNS_KEPT`].
    fn prune_runs(&self, process_id: &str) -> Result<(), ProcessError> {
        let keys = self.run_keys(process_id)?;
        if keys.len() <= MAX_PROCESS_RUNS_KEPT {
            return Ok(());
        }
        let cf = self.runs_cf()?;
        let excess = keys.len() - MAX_PROCESS_RUNS_KEPT;
        for key in keys.into_iter().take(excess) {
            self.db
                .delete_cf(&cf, key)
                .map_err(|e| ProcessError::Store(e.to_string()))?;
        }
        Ok(())
    }
}
