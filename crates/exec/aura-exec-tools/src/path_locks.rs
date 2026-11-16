//! One file-changing tool call per path at a time.
//!
//! `edit_file` reads a file, replaces text and writes the whole file back.
//! The agent loop runs a turn's tool calls concurrently, so two edits to
//! the same file would both read the original and the last write would
//! silently drop the other, while both reported success. Calls that change
//! a file hold that path's lock for the whole read-modify-write; calls on
//! different paths still run in parallel.
//!
//! The map is process-wide rather than per executor because separate
//! agents (and separate executors) can share a workspace.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, Weak};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

type LockMap = HashMap<PathBuf, Weak<AsyncMutex<()>>>;

static LOCKS: LazyLock<Mutex<LockMap>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Wait for exclusive use of `path`, a resolved path inside the sandbox.
/// The lock is released when the guard drops.
pub(crate) async fn lock_path(path: &Path) -> OwnedMutexGuard<()> {
    let lock = {
        let mut locks = LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
        locks.retain(|_, held| held.strong_count() > 0);
        if let Some(existing) = locks.get(path).and_then(Weak::upgrade) {
            existing
        } else {
            let fresh = Arc::new(AsyncMutex::new(()));
            locks.insert(path.to_path_buf(), Arc::downgrade(&fresh));
            fresh
        }
    };
    lock.lock_owned().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn same_path_waits_for_the_holder() {
        let path = PathBuf::from("/path-locks-test/same.txt");
        let held = lock_path(&path).await;
        let waiting = tokio::spawn({
            let path = path.clone();
            async move { lock_path(&path).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished(), "second caller must wait");
        drop(held);
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("second caller gets the lock once the first drops it")
            .unwrap();
    }

    #[tokio::test]
    async fn different_paths_do_not_block() {
        let _a = lock_path(Path::new("/path-locks-test/a.txt")).await;
        tokio::time::timeout(
            Duration::from_secs(1),
            lock_path(Path::new("/path-locks-test/b.txt")),
        )
        .await
        .expect("a different path is never blocked");
    }

    #[tokio::test]
    async fn released_locks_are_dropped_from_the_map() {
        let path = PathBuf::from("/path-locks-test/released.txt");
        drop(lock_path(&path).await);
        let _other = lock_path(Path::new("/path-locks-test/other.txt")).await;
        let locks = LOCKS.lock().unwrap();
        assert!(locks.get(&path).is_none_or(|held| held.strong_count() == 0));
    }
}
