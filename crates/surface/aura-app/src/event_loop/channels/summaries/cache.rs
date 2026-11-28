//! The summaries kept on disk, so a message is summarized once ever.
//!
//! # Invariants
//!
//! * Entries are keyed by the blake3 hex of the message text, so the same
//!   text sent twice, or seen again after a restart, is never summarized
//!   again.
//! * The file is read once, on first use, and written whole after new
//!   summaries came in: to a temporary file in the same folder, then
//!   renamed over the old one, so a crash leaves the old file or the new
//!   one, never half of either. Both run off the async runtime.
//! * At most [`MAX_ENTRIES`] are kept; the oldest go first.
//!
//! # Failure modes
//!
//! A missing file is an empty cache. An unreadable one is logged and
//! treated as empty; the next save replaces it. A failed save is logged:
//! the summaries stay in memory for this run.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing::warn;

/// File under the data directory that keeps the summaries.
pub(super) const CACHE_FILE: &str = "message_summaries.json";

/// Most summaries kept. A summary is about a hundred bytes, so the file
/// stays well under a megabyte.
pub(super) const MAX_ENTRIES: usize = 5_000;

/// The cache key of message `text`.
pub(super) fn text_key(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

/// One summary as kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Entry {
    pub(super) summary: String,
    /// When it was made, in Unix milliseconds; the oldest go first
    pub(super) at_ms: u64,
}

/// What the file holds.
#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    #[serde(default)]
    entries: HashMap<String, Entry>,
}

/// What the cache holds in memory.
#[derive(Debug, Default)]
struct State {
    /// Whether the file was read
    loaded: bool,
    entries: HashMap<String, Entry>,
    /// Keys being summarized now, so a second ask doesn't call again
    in_flight: HashSet<String>,
}

/// The summaries of the user's channel messages, shared by every request.
#[derive(Debug, Default)]
pub(crate) struct SummaryCache {
    state: Mutex<State>,
}

impl SummaryCache {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        // A panic while holding the lock leaves plain data behind; the
        // cache is still usable.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Read the file at `path` unless it was read already.
    pub(super) async fn ensure_loaded(&self, path: &Path) {
        if self.state().loaded {
            return;
        }
        let owned = path.to_path_buf();
        let entries = tokio::task::spawn_blocking(move || read_file(&owned))
            .await
            .unwrap_or_default();
        let mut state = self.state();
        if !state.loaded {
            state.loaded = true;
            for (key, entry) in entries {
                state.entries.entry(key).or_insert(entry);
            }
        }
    }

    /// The summary kept for `key`.
    pub(super) fn get(&self, key: &str) -> Option<String> {
        self.state().entries.get(key).map(|e| e.summary.clone())
    }

    /// Mark `key` as being summarized. `false` when it already is.
    pub(super) fn start(&self, key: &str) -> bool {
        self.state().in_flight.insert(key.to_string())
    }

    /// Done summarizing `key`: keep `summary` when there is one.
    pub(super) fn finish(&self, key: &str, summary: Option<&str>, at_ms: u64) {
        let mut state = self.state();
        state.in_flight.remove(key);
        if let Some(summary) = summary {
            let entry = Entry {
                summary: summary.to_string(),
                at_ms,
            };
            state.entries.insert(key.to_string(), entry);
            evict_oldest(&mut state.entries, MAX_ENTRIES);
        }
    }

    /// Write every summary to `path`, off the async runtime.
    pub(super) async fn save(&self, path: &Path) {
        let file = CacheFile {
            entries: self.state().entries.clone(),
        };
        let owned = path.to_path_buf();
        let written = tokio::task::spawn_blocking(move || write_file(&owned, &file)).await;
        match written {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                warn!(path = %path.display(), %error, "Could not save message summaries");
            }
            Err(error) => {
                warn!(path = %path.display(), %error, "Saving message summaries stopped");
            }
        }
    }

    /// How many summaries are kept.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.state().entries.len()
    }
}

/// Drop the oldest of `entries` until at most `max` are left.
pub(super) fn evict_oldest(entries: &mut HashMap<String, Entry>, max: usize) {
    if entries.len() <= max {
        return;
    }
    let mut by_age: Vec<(u64, String)> = entries
        .iter()
        .map(|(key, entry)| (entry.at_ms, key.clone()))
        .collect();
    by_age.sort_unstable();
    let excess = entries.len() - max;
    for (_, key) in by_age.into_iter().take(excess) {
        entries.remove(&key);
    }
}

/// The summaries in the file at `path`; none when it is missing or
/// unreadable (logged).
fn read_file(path: &Path) -> HashMap<String, Entry> {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<CacheFile>(&bytes) {
            Ok(file) => file.entries,
            Err(error) => {
                warn!(path = %path.display(), %error, "Ignoring unreadable message summaries");
                HashMap::new()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
        Err(error) => {
            warn!(path = %path.display(), %error, "Could not read message summaries");
            HashMap::new()
        }
    }
}

/// Write `file` to `path` through a temporary file renamed into place.
fn write_file(path: &Path, file: &CacheFile) -> std::io::Result<()> {
    let dir: PathBuf = path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    std::fs::create_dir_all(&dir)?;
    let bytes = serde_json::to_vec(file).map_err(std::io::Error::other)?;
    let mut temp = tempfile::NamedTempFile::new_in(&dir)?;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}
