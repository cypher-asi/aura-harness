//! Stopping a migration at a chosen step, for tests.
//!
//! A fault armed for a database path fails the next migration of that
//! path once, right after the step it names. Only tests arm one; outside
//! them [`check`] finds the list empty.

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

static ARMED: std::sync::Mutex<Vec<(std::path::PathBuf, FailAt)>> =
    std::sync::Mutex::new(Vec::new());

/// Fail the next migration of the database at `db_path` after `at`.
pub fn arm(db_path: &Path, at: FailAt) {
    ARMED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((db_path.to_path_buf(), at));
}

/// The armed fault for `db_path` at `at`, disarmed as it fires.
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
