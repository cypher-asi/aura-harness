//! The queue on disk, and the lock that keeps one processor per repo.
//!
//! ## Invariants
//!
//! - The queue file is replaced atomically: written to a temporary file
//!   in the same directory, synced, then renamed over the old one. A
//!   crash leaves the old queue or the new one, never half of either.
//! - A queue file that cannot be read as a queue is moved aside (never
//!   deleted) and the queue starts empty, so one bad write can't stop the
//!   repo's merges for good.
//! - While a processor holds [`ProcessorLock`], no other processor of the
//!   same repo runs. A lock whose holder stopped touching it for
//!   [`STALE_LOCK`] is taken over: its holder crashed.
//!
//! Everything here is blocking file IO; async callers run it on a
//! blocking thread.

use crate::error::QueueError;
use crate::state::QueueState;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Name of the queue file in the queue directory.
pub const QUEUE_FILE: &str = "merge-queue.json";
/// Name of the processor lock in the queue directory.
pub const LOCK_FILE: &str = "merge-queue.lock";
/// How long a lock may go untouched before it counts as abandoned.
pub const STALE_LOCK: Duration = Duration::from_secs(10 * 60);

/// A repo's queue file.
#[derive(Debug, Clone)]
pub struct QueueStore {
    dir: PathBuf,
}

impl QueueStore {
    /// The queue kept in `dir` (created on the first save).
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory the queue lives in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self) -> PathBuf {
        self.dir.join(QUEUE_FILE)
    }

    /// The stored queue: empty when there is none yet, or when the file
    /// was unreadable (moved aside; the second value says where).
    ///
    /// # Errors
    /// The file exists but can't be read, or can't be moved aside.
    pub fn load(&self) -> Result<(QueueState, Option<PathBuf>), QueueError> {
        let path = self.path();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok((QueueState::default(), None));
            }
            Err(e) => return Err(QueueError::io(&path, e)),
        };
        match serde_json::from_str(&text) {
            Ok(state) => Ok((state, None)),
            Err(_) => {
                let aside = self.dir.join(format!(
                    "{QUEUE_FILE}.corrupt-{}",
                    chrono::Utc::now().timestamp()
                ));
                fs::rename(&path, &aside).map_err(|e| QueueError::io(&path, e))?;
                Ok((QueueState::default(), Some(aside)))
            }
        }
    }

    /// Replace the stored queue with `state`, atomically.
    ///
    /// # Errors
    /// The directory or file can't be written.
    pub fn save(&self, state: &QueueState) -> Result<(), QueueError> {
        fs::create_dir_all(&self.dir).map_err(|e| QueueError::io(&self.dir, e))?;
        let json = serde_json::to_vec_pretty(state)
            .map_err(|e| QueueError::Corrupt(format!("encoding the queue: {e}")))?;
        let tmp = self.dir.join(format!("{QUEUE_FILE}.tmp"));
        let mut file = fs::File::create(&tmp).map_err(|e| QueueError::io(&tmp, e))?;
        file.write_all(&json).map_err(|e| QueueError::io(&tmp, e))?;
        file.sync_all().map_err(|e| QueueError::io(&tmp, e))?;
        drop(file);
        let path = self.path();
        fs::rename(&tmp, &path).map_err(|e| QueueError::io(&path, e))
    }
}

/// The right to process one repo's queue, held while a lock file exists.
/// Dropping it removes the file.
#[derive(Debug)]
pub struct ProcessorLock {
    path: PathBuf,
}

impl ProcessorLock {
    /// Take the lock in `dir`; `None` when a live processor holds it.
    ///
    /// # Errors
    /// The lock file can't be created or inspected.
    pub fn acquire(dir: &Path) -> Result<Option<Self>, QueueError> {
        fs::create_dir_all(dir).map_err(|e| QueueError::io(dir, e))?;
        let path = dir.join(LOCK_FILE);
        for _ in 0..2 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    // The holder is for people reading the file; nothing
                    // parses it, so a failed write is no reason to give up.
                    let _ = writeln!(file, "pid {}", std::process::id());
                    return Ok(Some(Self { path }));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if !is_stale(&path)? {
                        return Ok(None);
                    }
                    tracing::warn!(lock = %path.display(), "Taking over an abandoned merge queue lock");
                    remove_if_present(&path)?;
                }
                Err(e) => return Err(QueueError::io(&path, e)),
            }
        }
        Ok(None)
    }

    /// Show the lock is still held, so no one takes it over.
    ///
    /// # Errors
    /// The lock file can't be written.
    pub fn touch(&self) -> Result<(), QueueError> {
        fs::write(&self.path, format!("pid {}\n", std::process::id()))
            .map_err(|e| QueueError::io(&self.path, e))
    }
}

impl Drop for ProcessorLock {
    fn drop(&mut self) {
        // Best effort: a lock left behind goes stale and is taken over.
        let _ = fs::remove_file(&self.path);
    }
}

fn is_stale(path: &Path) -> Result<bool, QueueError> {
    let modified = match fs::metadata(path).and_then(|meta| meta.modified()) {
        Ok(modified) => modified,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(QueueError::io(path, e)),
    };
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    Ok(age > STALE_LOCK)
}

fn remove_if_present(path: &Path) -> Result<(), QueueError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(QueueError::io(path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{MergeRequest, Origin, QueuedPush};

    fn one_entry() -> QueueState {
        let mut state = QueueState::default();
        state.enqueue(
            MergeRequest {
                push: QueuedPush {
                    branch: "aura/a".into(),
                    head: "h".into(),
                    remote: None,
                },
                origin: Origin::default(),
            },
            chrono::Utc::now(),
        );
        state
    }

    #[test]
    fn a_missing_queue_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let (state, aside) = QueueStore::new(dir.path().join("q")).load().unwrap();
        assert_eq!(state, QueueState::default());
        assert_eq!(aside, None);
    }

    #[test]
    fn a_saved_queue_loads_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = QueueStore::new(dir.path().join("q"));
        let state = one_entry();
        store.save(&state).unwrap();
        assert_eq!(store.load().unwrap().0, state);
        assert!(!store.dir().join(format!("{QUEUE_FILE}.tmp")).exists());
    }

    #[test]
    fn an_unreadable_queue_is_moved_aside_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        let store = QueueStore::new(dir.path());
        fs::write(dir.path().join(QUEUE_FILE), "{ half a").unwrap();
        let (state, aside) = store.load().unwrap();
        assert_eq!(state, QueueState::default());
        let aside = aside.unwrap();
        assert_eq!(fs::read_to_string(aside).unwrap(), "{ half a");
    }

    #[test]
    fn one_processor_holds_the_lock_until_it_drops_it() {
        let dir = tempfile::tempdir().unwrap();
        let first = ProcessorLock::acquire(dir.path()).unwrap();
        assert!(first.is_some());
        assert!(ProcessorLock::acquire(dir.path()).unwrap().is_none());
        drop(first);
        assert!(ProcessorLock::acquire(dir.path()).unwrap().is_some());
    }
}
