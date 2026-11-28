//! Stopping a migration at a chosen step, for tests.
//!
//! With `test-support` (or in this crate's tests) a fault armed for a
//! database path fails the next migration of that path once, right
//! after the step it names. Without them [`check`] compiles to nothing.

use std::path::Path;

use super::SealMigrationError;

/// A step of the migration a test can stop it after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailAt {
    /// The sealed copy is written; nothing is swapped yet.
    AfterCopy,
    /// The plaintext database has moved to the backup path.
    AfterBackupRename,
    /// The sealed copy is in place; the marker is not written.
    AfterSwap,
    /// The marker is written; the backup still exists.
    AfterMarker,
}

#[cfg(any(test, feature = "test-support"))]
static ARMED: std::sync::Mutex<Vec<(std::path::PathBuf, FailAt)>> =
    std::sync::Mutex::new(Vec::new());

/// Fail the next migration of the database at `db_path` after `at`.
#[cfg(any(test, feature = "test-support"))]
pub fn arm(db_path: &Path, at: FailAt) {
    ARMED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((db_path.to_path_buf(), at));
}

/// The armed fault for `db_path` at `at`, disarmed as it fires.
#[cfg(any(test, feature = "test-support"))]
pub(super) fn check(db_path: &Path, at: FailAt) -> Result<(), SealMigrationError> {
    let mut armed = ARMED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match armed
        .iter()
        .position(|(path, step)| path == db_path && *step == at)
    {
        Some(index) => {
            armed.remove(index);
            Err(SealMigrationError::Interrupted(at))
        }
        None => Ok(()),
    }
}

#[cfg(not(any(test, feature = "test-support")))]
#[inline]
#[allow(clippy::unnecessary_wraps)] // Mirrors the test build's signature.
pub(super) const fn check(_db_path: &Path, _at: FailAt) -> Result<(), SealMigrationError> {
    Ok(())
}
