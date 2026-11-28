//! An exclusive lock on a file, held by the operating system for as long
//! as the [`FileLock`] lives and released when it is dropped or its
//! process dies, however it dies. Nothing has to refresh it, and no one
//! can take it over from a holder that is still alive.
//!
//! - Unix: `flock(LOCK_EX)` on the open file.
//! - Windows: the file opened with no sharing, which no other handle can
//!   open until it is closed.
//!
//! Everything here is blocking; async callers run it on a blocking
//! thread.

use crate::error::QueueError;
use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

/// How often [`FileLock::exclusive`] tries a held lock again.
const RETRY: Duration = Duration::from_millis(20);

/// An exclusive lock on one file.
#[derive(Debug)]
pub struct FileLock {
    // Held for the lock: closing the file releases it.
    _file: File,
}

impl FileLock {
    /// Take the lock on `path` (created if missing); `None` while another
    /// holder, in this process or another, has it.
    ///
    /// # Errors
    /// The file can't be created or locked for another reason.
    pub fn try_exclusive(path: &Path) -> Result<Option<Self>, QueueError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| QueueError::io(dir, e))?;
        }
        Ok(os::try_lock(path)
            .map_err(|e| QueueError::io(path, e))?
            .map(|file| Self { _file: file }))
    }

    /// Take the lock on `path`, waiting up to `wait` for its holder.
    ///
    /// # Errors
    /// It is still held after `wait`, or it can't be taken.
    pub fn exclusive(path: &Path, wait: Duration) -> Result<Self, QueueError> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(lock) = Self::try_exclusive(path)? {
                return Ok(lock);
            }
            if Instant::now() >= deadline {
                return Err(QueueError::Io {
                    path: path.display().to_string(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("still locked after {}s", wait.as_secs()),
                    ),
                });
            }
            std::thread::sleep(RETRY);
        }
    }
}

#[cfg(unix)]
mod os {
    use rustix::fs::{flock, FlockOperation};
    use std::fs::{File, OpenOptions};
    use std::path::Path;

    pub(super) fn try_lock(path: &Path) -> std::io::Result<Option<File>> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Some(file)),
            Err(e) if e == rustix::io::Errno::WOULDBLOCK => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(windows)]
mod os {
    use std::fs::{File, OpenOptions};
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::Path;

    /// `ERROR_SHARING_VIOLATION`: another handle has the file open.
    const SHARING_VIOLATION: i32 = 32;

    pub(super) fn try_lock(path: &Path) -> std::io::Result<Option<File>> {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(path)
        {
            Ok(file) => Ok(Some(file)),
            Err(e) if e.raw_os_error() == Some(SHARING_VIOLATION) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod os {
    use std::fs::File;
    use std::path::Path;

    pub(super) fn try_lock(path: &Path) -> std::io::Result<Option<File>> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!("no file locks on this platform for {}", path.display()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_at_a_time_until_it_drops_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.lock");
        let first = FileLock::try_exclusive(&path).unwrap();
        assert!(first.is_some());
        assert!(FileLock::try_exclusive(&path).unwrap().is_none());
        drop(first);
        assert!(FileLock::try_exclusive(&path).unwrap().is_some());
    }

    #[test]
    fn waiting_for_a_held_lock_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.lock");
        let _held = FileLock::try_exclusive(&path).unwrap();
        let waited = FileLock::exclusive(&path, Duration::from_millis(60));
        assert!(waited.is_err());
    }

    #[test]
    fn a_lock_is_taken_as_soon_as_its_holder_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.lock");
        let held = FileLock::try_exclusive(&path).unwrap();
        let waiter = {
            let path = path.clone();
            std::thread::spawn(move || FileLock::exclusive(&path, Duration::from_secs(10)).is_ok())
        };
        std::thread::sleep(Duration::from_millis(50));
        drop(held);
        assert!(waiter.join().unwrap());
    }
}
