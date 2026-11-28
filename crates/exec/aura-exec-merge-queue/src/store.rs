//! The queue on disk, and the locks that keep it consistent.
//!
//! ## Invariants
//!
//! - The queue file is replaced atomically: written to a temporary file
//!   in the same directory, synced, then renamed over the old one. A
//!   crash leaves the old queue or the new one, never half of either.
//! - Every change is a read, modify, write under [`STATE_LOCK_FILE`]'s
//!   lock ([`QueueStore::update`]), so processes sharing a queue never
//!   overwrite each other's changes.
//! - While a processor holds [`ProcessorLock`], no other processor of the
//!   same repo runs, in this process or another. The lock is the
//!   operating system's ([`FileLock`]): it lasts as long as its holder
//!   and goes with it, so nothing refreshes it and no one takes it over
//!   from a live holder, however long a merge runs.
//! - Every entry a change finishes is appended to the history
//!   ([`crate::history`]) under the same lock, before the queue file is
//!   saved, so pruning the queue file never loses a merge.
//! - A queue file that cannot be read as a queue is moved aside (never
//!   deleted) and the queue starts empty, so one bad write can't stop the
//!   repo's merges for good.
//!
//! Everything here is blocking file IO; async callers run it on a
//! blocking thread.

use crate::entry::MergeEntry;
use crate::error::QueueError;
use crate::lock::FileLock;
use crate::state::QueueState;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Name of the queue file in the queue directory.
pub const QUEUE_FILE: &str = "merge-queue.json";
/// Name of the processor lock in the queue directory.
pub const LOCK_FILE: &str = "merge-queue.lock";
/// Name of the lock every change to the queue file is made under.
pub const STATE_LOCK_FILE: &str = "merge-queue.state.lock";
/// Longest a change waits for another process's change to the file.
pub const STATE_LOCK_WAIT: Duration = Duration::from_secs(30);

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

    /// Apply `edit` to the stored queue and save it when it changed, all
    /// under the state lock. Returns the queue after the edit and what
    /// `edit` returned.
    ///
    /// # Errors
    /// The lock can't be taken in [`STATE_LOCK_WAIT`], or the file can't
    /// be read or written; nothing is changed then.
    pub fn update<T>(
        &self,
        edit: impl FnOnce(&mut QueueState) -> T,
    ) -> Result<(QueueState, T), QueueError> {
        let _lock = FileLock::exclusive(&self.dir.join(STATE_LOCK_FILE), STATE_LOCK_WAIT)?;
        let (before, aside) = self.load()?;
        if let Some(aside) = aside {
            tracing::warn!(moved_to = %aside.display(), "The merge queue file was unreadable; started a new one");
        }
        let mut after = before.clone();
        let result = edit(&mut after);
        if after != before {
            // The history first: a merge it lacks would be lost once
            // the queue file prunes it.
            crate::history::append(&self.dir, &newly_finished(&before, &after))?;
            self.save(&after)?;
        }
        Ok((after, result))
    }

    /// Start the history from the finished entries an older queue file
    /// kept, when there is no history yet, under the state lock.
    ///
    /// # Errors
    /// The lock can't be taken, or a file can't be read or written.
    pub fn seed_history(&self) -> Result<(), QueueError> {
        if crate::history::path_in(&self.dir).exists() {
            return Ok(());
        }
        let _lock = FileLock::exclusive(&self.dir.join(STATE_LOCK_FILE), STATE_LOCK_WAIT)?;
        let (state, _) = self.load()?;
        let finished: Vec<MergeEntry> = state
            .entries
            .into_iter()
            .filter(|entry| entry.status.is_finished())
            .collect();
        crate::history::seed(&self.dir, &finished)
    }
}

/// The entries `after` has finished that `before` had not, in `after`'s
/// order.
fn newly_finished(before: &QueueState, after: &QueueState) -> Vec<MergeEntry> {
    after
        .entries
        .iter()
        .filter(|entry| entry.status.is_finished())
        .filter(|entry| {
            !before
                .entries
                .iter()
                .any(|old| old.id == entry.id && old.status.is_finished())
        })
        .cloned()
        .collect()
}

/// The right to process one repo's queue, for as long as it lives.
#[derive(Debug)]
pub struct ProcessorLock {
    _lock: FileLock,
}

impl ProcessorLock {
    /// Take the lock in `dir`; `None` while a live processor, in this
    /// process or another, holds it.
    ///
    /// # Errors
    /// The lock file can't be created or locked.
    pub fn acquire(dir: &Path) -> Result<Option<Self>, QueueError> {
        Ok(FileLock::try_exclusive(&dir.join(LOCK_FILE))?.map(|lock| Self { _lock: lock }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{EntryId, MergeRequest, Origin, QueuedPush};

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
    fn updates_from_two_stores_of_one_queue_both_land() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (QueueStore::new(dir.path()), QueueStore::new(dir.path()));
        let push = |branch: &str| MergeRequest {
            push: QueuedPush {
                branch: branch.into(),
                head: "h".into(),
                remote: None,
            },
            origin: Origin::default(),
        };
        a.update(|state| state.enqueue(push("aura/a"), chrono::Utc::now()))
            .unwrap();
        let (after, _) = b
            .update(|state| state.enqueue(push("aura/b"), chrono::Utc::now()))
            .unwrap();
        let branches: Vec<&str> = after.entries.iter().map(|e| e.branch.as_str()).collect();
        assert_eq!(branches, ["aura/a", "aura/b"]);
        assert_eq!(a.load().unwrap().0, after);
    }

    #[test]
    fn every_finished_entry_reaches_the_history_though_the_file_prunes_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = QueueStore::new(dir.path());
        for n in 0..5 {
            store
                .update(|state| {
                    let push = MergeRequest {
                        push: QueuedPush {
                            branch: format!("aura/{n}"),
                            head: "h".into(),
                            remote: None,
                        },
                        origin: Origin::default(),
                    };
                    let now = chrono::Utc::now();
                    let id = state.enqueue(push, now).id().clone();
                    state.claim_next(now);
                    state.finish(&id, crate::state::RunOutcome::Failed("no".into()), now);
                    state.prune(2);
                })
                .unwrap();
        }
        store.update(|_| ()).unwrap();
        assert_eq!(store.load().unwrap().0.entries.len(), 2, "the file prunes");
        let page = crate::history::read_page(dir.path(), None, 10).unwrap();
        let ids: Vec<&str> = page.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            ["q5", "q4", "q3", "q2", "q1"],
            "each once, newest first"
        );
    }

    #[test]
    fn the_history_starts_from_an_old_queue_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = QueueStore::new(dir.path());
        let mut state = one_entry();
        let now = chrono::Utc::now();
        state.claim_next(now);
        state.finish(
            &EntryId::from_seq(1),
            crate::state::RunOutcome::Failed("x".into()),
            now,
        );
        store.save(&state).unwrap();
        store.seed_history().unwrap();
        store.seed_history().unwrap();
        let page = crate::history::read_page(dir.path(), None, 10).unwrap();
        assert_eq!(page.entries.len(), 1);
    }

    #[test]
    fn an_unchanged_queue_is_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let store = QueueStore::new(dir.path());
        store.update(|_| ()).unwrap();
        assert!(!dir.path().join(QUEUE_FILE).exists());
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
