//! The app's local store, encrypted at rest.
//!
//! The terminal, `aura -p` and the desktop app keep channel posts,
//! prompts, summaries and the agent registry in one RocksDB database
//! under the data directory. [`open`] opens it **sealed**: values are
//! encrypted with `aura-node`'s AES-256-GCM [`SealCipher`], keys and
//! counters stay plaintext (see [`aura_store_db::seal`]). The key is
//! kept as described in [`keys`].
//!
//! # First start with an unencrypted store
//!
//! The data is copied into a new sealed database, the copy is verified
//! (every key of every column family read back), and only then swapped
//! in; the plaintext copy is then deleted. See
//! [`aura_store_db::seal_migration`] for the steps and crash recovery.
//! If anything fails before the swap is proven, the original is back in
//! place, the store opens **unencrypted** with a warning, and the next
//! start tries again.
//!
//! # Invariants
//!
//! * Once the `.aura-sealed` marker exists, the store only ever opens
//!   sealed. A missing or wrong key is an error naming where the key was
//!   looked for, never a new key or an empty store.
//! * The work runs on a blocking thread, under an OS file lock on
//!   `<data_dir>/.aura-seal.lock`, so two processes never migrate the
//!   same directory at once.

mod error;
pub mod keys;
#[cfg(test)]
mod tests;

pub use error::LocalStoreError;
pub use keys::KeySource;

use anyhow::Context as _;
use aura_exec_merge_queue::FileLock;
use aura_store_db::seal_migration::{self, marker_path, SealMigrationError};
use aura_store_db::{RocksStore, SealCipher};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// The lock file guarding a data directory while its store is prepared.
const LOCK_FILENAME: &str = ".aura-seal.lock";

/// How long to wait for another process preparing the same directory.
const LOCK_WAIT: Duration = Duration::from_secs(120);

/// Open the store at `db_path` in `data_dir` sealed, encrypting an
/// unencrypted one first. Runs on a blocking thread.
///
/// # Errors
/// A missing or wrong key for an encrypted store, an original that
/// could not be restored after a failed migration, and RocksDB open
/// failures (for example another process using the store).
pub async fn open(data_dir: &Path, db_path: &Path) -> anyhow::Result<Arc<RocksStore>> {
    let data_dir = data_dir.to_path_buf();
    let db_path = db_path.to_path_buf();
    let store = tokio::task::spawn_blocking(move || {
        let _lock = lock(&data_dir)?;
        let keys = KeySource::for_data_dir(&data_dir)?;
        open_with(&data_dir, &db_path, &keys)
    })
    .await
    .context("the task opening the database stopped")??;
    Ok(Arc::new(store))
}

/// Hold the data directory's preparation lock.
fn lock(data_dir: &Path) -> Result<FileLock, LocalStoreError> {
    let path = data_dir.join(LOCK_FILENAME);
    FileLock::exclusive(&path, LOCK_WAIT).map_err(|e| LocalStoreError::Locked {
        path,
        detail: e.to_string(),
    })
}

/// Open sealed with the key from `keys`, or unencrypted when sealing the
/// store failed in a way that left the original untouched. The caller
/// holds the directory's lock.
///
/// # Errors
/// See [`open`].
pub fn open_with(
    data_dir: &Path,
    db_path: &Path,
    keys: &KeySource,
) -> Result<RocksStore, LocalStoreError> {
    let was_sealed = marker_path(data_dir).exists();
    match open_sealed(data_dir, db_path, keys, was_sealed) {
        Ok(store) => Ok(store),
        Err(e) if !was_sealed && !e.is_fatal() && !marker_path(data_dir).exists() => {
            warn!(
                db = %db_path.display(),
                error = %e,
                "could not encrypt the database; keeping it unencrypted for now, will retry next start"
            );
            RocksStore::open(db_path, false).map_err(|source| LocalStoreError::Open {
                db: db_path.to_path_buf(),
                source,
            })
        }
        Err(e) => Err(e),
    }
}

/// The key, any migration or cleanup, then the sealed open.
fn open_sealed(
    data_dir: &Path,
    db_path: &Path,
    keys: &KeySource,
    was_sealed: bool,
) -> Result<RocksStore, LocalStoreError> {
    let key = match keys.load()? {
        Some(key) => key,
        None if was_sealed => return Err(missing_key(db_path, keys)),
        None => {
            info!(key = %keys.describe(), "creating the database key");
            keys.create()?
        }
    };
    let cipher = Arc::new(SealCipher::new(&key));
    drop(key);

    seal_migration::migrate_if_needed(data_dir, db_path, None, &cipher)
        .map_err(|e| migration_error(e, db_path, keys))?;
    if !marker_path(data_dir).exists() {
        // Fresh store: mark it sealed before anything is written to it.
        seal_migration::write_marker(data_dir, None, &cipher)
            .map_err(|e| migration_error(e, db_path, keys))?;
    }
    let store = RocksStore::open_sealed(db_path, false, Some(cipher)).map_err(|source| {
        LocalStoreError::Open {
            db: db_path.to_path_buf(),
            source,
        }
    })?;
    info!(db = %db_path.display(), key = %keys.describe(), "database opened encrypted");
    Ok(store)
}

fn missing_key(db_path: &Path, keys: &KeySource) -> LocalStoreError {
    LocalStoreError::MissingKey {
        db: db_path.to_path_buf(),
        location: keys.describe(),
    }
}

/// A wrong key reads better with where the key came from.
fn migration_error(e: SealMigrationError, db_path: &Path, keys: &KeySource) -> LocalStoreError {
    match e {
        SealMigrationError::WrongKey { .. } | SealMigrationError::ForeignSealedValue { .. } => {
            LocalStoreError::WrongKey {
                db: db_path.to_path_buf(),
                location: keys.describe(),
            }
        }
        other => LocalStoreError::Migration(other),
    }
}
