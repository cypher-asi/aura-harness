//! Atomic, resumable encrypt-in-place migration of a store's database.
//!
//! [`migrate_if_needed`] turns a **plaintext** database into a sealed one
//! before the store is opened: copy, verify, swap. Plaintext data is
//! never changed in place and is never removed before a verified sealed
//! copy has taken its place. Used by `aura-node`'s sealed boot and by the
//! local app.
//!
//! # Directory layout
//!
//! For a database directory `<db>` (e.g. `$data_dir/db`):
//!
//! * `<db>.sealed-migrating`: the sealed copy while it is written and
//!   verified. Disposable: deleted and rebuilt on any retry.
//! * `<db>.plaintext-backup`: the original database between the swap
//!   and the final cleanup. The authoritative data while it exists.
//! * `$data_dir/.aura-sealed`: the non-secret marker "this store is
//!   sealed", with a key check (see [`marker`]).
//!
//! # Steps
//!
//! ```text
//!  (1) open <db> read-write (holds its lock: nobody else is using it)
//!      seal_db_copy(<db> -> <db>.sealed-migrating)   [flushed]
//!      verify: same keys in every column family, every value read back
//!      verify the copy opens as a sealed store; release <db>
//!  (2) rename <db> -> <db>.plaintext-backup
//!  (3) rename <db>.sealed-migrating -> <db>
//!  (4) write the .aura-sealed marker
//!  (5) the live <db> opens and the key check passes; delete the backup
//! ```
//!
//! A failure in (2) to (5) is rolled back at once: the marker goes, the
//! backup is renamed back to `<db>`, the copy is deleted, and the error
//! is returned, so the caller finds its original data where it was.
//!
//! # Crash recovery on the next start
//!
//! * **No marker, backup present** (crash in (2) to (4)): the backup is
//!   the data; whatever is at `<db>` is a copy. Delete it, rename the
//!   backup back, migrate again from scratch.
//! * **No marker, temp present** (crash in (1)): delete the partial
//!   copy and start again. The source was never touched.
//! * **Marker present, backup present** (crash in (5)): the sealed
//!   database is live; check it opens, then delete the backup. If there
//!   is no database at `<db>` the marker is stale: drop it and recover
//!   as above.
//! * **Marker present, nothing else**: steady state.

mod error;
mod fault;
pub mod marker;
#[cfg(test)]
mod tests;
mod verify;

pub use error::SealMigrationError;
#[doc(hidden)]
pub use fault::arm as arm_fault;
pub use fault::FailAt;
pub use marker::SEALED_MARKER_FILENAME;
pub use marker::{check_key, marker_path, read_marker, write_marker, SealedMarker};
pub use verify::verify_sealed_copy;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rocksdb::Options;
use tracing::{info, warn};

use crate::error::backend;
use crate::migrate::{seal_db_copy_from, Db, SealCopyStats};
use crate::seal::SealCipher;
use crate::RocksStore;

/// Suffix of the temp directory the sealed copy is written into.
pub const MIGRATING_SUFFIX: &str = "sealed-migrating";

/// Suffix of the plaintext backup directory kept across the swap.
pub const BACKUP_SUFFIX: &str = "plaintext-backup";

/// What [`migrate_if_needed`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// Nothing to migrate (fresh state dir, or already sealed), and why.
    NotNeeded(&'static str),
    /// Plaintext state was converted to sealed.
    Migrated(SealCopyStats),
}

/// `<db>.sealed-migrating` next to the database directory.
#[must_use]
pub fn migrating_dir(db_path: &Path) -> PathBuf {
    sibling(db_path, MIGRATING_SUFFIX)
}

/// `<db>.plaintext-backup` next to the database directory.
#[must_use]
pub fn backup_dir(db_path: &Path) -> PathBuf {
    sibling(db_path, BACKUP_SUFFIX)
}

fn sibling(db_path: &Path, suffix: &str) -> PathBuf {
    let name = db_path
        .file_name()
        .map_or_else(|| "state".to_string(), |n| n.to_string_lossy().into_owned());
    db_path.with_file_name(format!("{name}.{suffix}"))
}

/// Whether a database with data exists at `path`. `CURRENT` is written
/// when a database is created, so without it there is nothing to keep.
#[must_use]
pub fn db_has_data(path: &Path) -> bool {
    path.join("CURRENT").is_file()
}

/// The paths one migration works on.
struct Paths<'a> {
    data_dir: &'a Path,
    db: &'a Path,
    temp: PathBuf,
    backup: PathBuf,
}

impl<'a> Paths<'a> {
    fn new(data_dir: &'a Path, db: &'a Path) -> Self {
        Self {
            data_dir,
            db,
            temp: migrating_dir(db),
            backup: backup_dir(db),
        }
    }
}

/// Seal the database at `db_path` if it holds plaintext data, and clear
/// what an interrupted earlier run left behind. Call it after the key is
/// known and before the store is opened.
///
/// It does not write the marker for a fresh (empty) database; the
/// caller does that with [`write_marker`] before opening it sealed.
///
/// # Errors
/// [`SealMigrationError`]. Unless [`SealMigrationError::is_fatal`], the
/// original database is back at `db_path` and no marker claims it is
/// sealed, so the caller may use it as it is and retry later.
pub fn migrate_if_needed(
    data_dir: &Path,
    db_path: &Path,
    key_id: Option<&str>,
    cipher: &Arc<SealCipher>,
) -> Result<MigrationOutcome, SealMigrationError> {
    let paths = Paths::new(data_dir, db_path);
    if marker_path(data_dir).exists() {
        if paths.backup.exists() && !db_has_data(db_path) {
            warn!(
                backup = %paths.backup.display(),
                "sealed marker without a database next to a plaintext backup; recovering the backup"
            );
            marker::remove_marker(data_dir)
                .map_err(|e| SealMigrationError::io("removing a stale marker", data_dir, e))?;
        } else {
            check_key(data_dir, cipher)?;
            finish_sealed_state(&paths, cipher)?;
            return Ok(MigrationOutcome::NotNeeded("state is already sealed"));
        }
    }

    recover_interrupted_migration(&paths)?;
    if !db_has_data(db_path) {
        return Ok(MigrationOutcome::NotNeeded(
            "fresh state (nothing to migrate)",
        ));
    }

    info!(db = %db_path.display(), "unencrypted database found; sealing a copy of it");
    let stats = seal_and_swap(&paths, key_id, cipher)?;
    info!(
        db = %db_path.display(),
        values_sealed = stats.values_sealed,
        values_copied = stats.values_copied,
        "database encrypted"
    );
    Ok(MigrationOutcome::Migrated(stats))
}

/// Already sealed: clear leftovers of an interrupted migration.
fn finish_sealed_state(
    paths: &Paths<'_>,
    cipher: &Arc<SealCipher>,
) -> Result<(), SealMigrationError> {
    if paths.temp.exists() {
        warn!(temp = %paths.temp.display(), "removing a stale migration temp dir");
        remove_dir(&paths.temp, "removing a stale migration temp dir")?;
    }
    if paths.backup.exists() {
        // Crash in (5): drop the plaintext only once the live sealed
        // database provably opens.
        verify_sealed_open(paths.db, cipher)?;
        drop_backup(paths);
    }
    Ok(())
}

/// No marker: roll back a mid-swap migration and drop partial copies.
fn recover_interrupted_migration(paths: &Paths<'_>) -> Result<(), SealMigrationError> {
    if paths.backup.exists() {
        warn!(
            backup = %paths.backup.display(),
            "interrupted migration found; restoring the unencrypted backup"
        );
        if paths.db.exists() {
            remove_dir(paths.db, "removing a partial sealed database")?;
        }
        std::fs::rename(&paths.backup, paths.db).map_err(|e| {
            SealMigrationError::io("restoring the unencrypted backup", &paths.backup, e)
        })?;
        fsync_parent(paths.db);
    }
    if paths.temp.exists() {
        warn!(temp = %paths.temp.display(), "removing a partial sealed copy; starting again");
        remove_dir(&paths.temp, "removing a partial sealed copy")?;
    }
    Ok(())
}

/// (1) to (5): copy and verify, swap, mark, drop the backup.
fn seal_and_swap(
    paths: &Paths<'_>,
    key_id: Option<&str>,
    cipher: &Arc<SealCipher>,
) -> Result<SealCopyStats, SealMigrationError> {
    let stats = copy_and_verify(paths, cipher).inspect_err(|_| discard_copy(paths))?;
    std::fs::rename(paths.db, &paths.backup)
        .map_err(|e| SealMigrationError::io("moving the unencrypted database aside", paths.db, e))
        .inspect_err(|_| discard_copy(paths))?;
    if let Err(cause) = swap_in(paths, key_id, cipher) {
        return Err(roll_back(paths, cause));
    }
    drop_backup(paths);
    Ok(stats)
}

/// (1): the sealed copy, verified, while holding the source's lock.
fn copy_and_verify(
    paths: &Paths<'_>,
    cipher: &Arc<SealCipher>,
) -> Result<SealCopyStats, SealMigrationError> {
    let locking = "opening the unencrypted database (is another aura process using it?)";
    let cf_names = Db::list_cf(&Options::default(), paths.db)
        .map_err(|e| SealMigrationError::store(locking, paths.db, backend(e)))?;
    let source = Db::open_cf(&Options::default(), paths.db, &cf_names)
        .map_err(|e| SealMigrationError::store(locking, paths.db, backend(e)))?;

    let stats = seal_db_copy_from(&source, &cf_names, &paths.temp, cipher)
        .map_err(|e| SealMigrationError::store("writing the sealed copy", &paths.temp, e))?;
    fault::check(paths.db, FailAt::AfterCopy)?;
    let src = verify::Source {
        db: &source,
        path: paths.db,
        cf_names: &cf_names,
    };
    let keys = verify::verify_against(&src, &paths.temp, cipher)?;
    if keys != stats.values_sealed + stats.values_copied {
        return Err(SealMigrationError::mismatch(
            &paths.temp,
            format!(
                "verified {keys} keys but copied {}",
                stats.values_sealed + stats.values_copied
            ),
        ));
    }
    verify_sealed_open(&paths.temp, cipher)?;
    info!(keys, copy = %paths.temp.display(), "sealed copy verified");
    Ok(stats)
}

/// (3) to (5), up to but not including dropping the backup.
fn swap_in(
    paths: &Paths<'_>,
    key_id: Option<&str>,
    cipher: &Arc<SealCipher>,
) -> Result<(), SealMigrationError> {
    fault::check(paths.db, FailAt::AfterBackupRename)?;
    std::fs::rename(&paths.temp, paths.db)
        .map_err(|e| SealMigrationError::io("moving the sealed copy into place", &paths.temp, e))?;
    fsync_parent(paths.db);
    fault::check(paths.db, FailAt::AfterSwap)?;
    write_marker(paths.data_dir, key_id, cipher)?;
    fault::check(paths.db, FailAt::AfterMarker)?;
    verify_sealed_open(paths.db, cipher)?;
    check_key(paths.data_dir, cipher)
}

/// Put the original database back after `cause`; returns the error to
/// report.
fn roll_back(paths: &Paths<'_>, cause: SealMigrationError) -> SealMigrationError {
    match restore_backup(paths) {
        Ok(()) => {
            warn!(
                db = %paths.db.display(),
                error = %cause,
                "encrypting the database failed; the unencrypted original is back in place"
            );
            cause
        }
        Err(source) => SealMigrationError::RollbackFailed {
            cause: Box::new(cause),
            backup: paths.backup.clone(),
            source,
        },
    }
}

fn restore_backup(paths: &Paths<'_>) -> std::io::Result<()> {
    if !db_has_data(&paths.backup) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the backup holds no database",
        ));
    }
    marker::remove_marker(paths.data_dir)?;
    if paths.db.exists() {
        // Only the sealed copy can be here: the original is the backup.
        std::fs::remove_dir_all(paths.db)?;
    }
    std::fs::rename(&paths.backup, paths.db)?;
    if paths.temp.exists() {
        std::fs::remove_dir_all(&paths.temp)?;
    }
    fsync_parent(paths.db);
    Ok(())
}

/// Delete the sealed copy after a failure before the swap; a leftover
/// is removed on the next start anyway.
fn discard_copy(paths: &Paths<'_>) {
    if paths.temp.exists() {
        if let Err(e) = std::fs::remove_dir_all(&paths.temp) {
            warn!(temp = %paths.temp.display(), error = %e, "could not remove the sealed copy");
        }
    }
}

/// (5): the plaintext backup goes once the sealed database is proven.
/// A failure only delays it: the next start finishes the cleanup.
fn drop_backup(paths: &Paths<'_>) {
    match std::fs::remove_dir_all(&paths.backup) {
        Ok(()) => info!(
            backup = %paths.backup.display(),
            "removed the unencrypted copy of the database"
        ),
        Err(e) => warn!(
            backup = %paths.backup.display(),
            error = %e,
            "could not remove the unencrypted copy of the database; will retry next start"
        ),
    }
}

/// Open the database at `path` sealed and close it again: proves RocksDB
/// accepts it before any plaintext is given up. Never creates one.
fn verify_sealed_open(path: &Path, cipher: &Arc<SealCipher>) -> Result<(), SealMigrationError> {
    if !db_has_data(path) {
        return Err(SealMigrationError::mismatch(
            path,
            "no database there".to_string(),
        ));
    }
    RocksStore::open_sealed(path, false, Some(Arc::clone(cipher)))
        .map(drop)
        .map_err(|e| SealMigrationError::store("opening the sealed database", path, e))
}

fn remove_dir(path: &Path, step: &'static str) -> Result<(), SealMigrationError> {
    std::fs::remove_dir_all(path).map_err(|e| SealMigrationError::io(step, path, e))
}

fn fsync_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        fsync_dir_best_effort(parent);
    }
}

/// Best-effort directory fsync so renames are durable (no-op off Unix;
/// NTFS metadata operations are journaled).
pub(crate) fn fsync_dir_best_effort(dir: &Path) {
    #[cfg(unix)]
    if let Ok(f) = std::fs::File::open(dir) {
        // Best effort by design: the rename already happened.
        let _ = f.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}
