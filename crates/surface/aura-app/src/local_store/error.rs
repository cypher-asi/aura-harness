//! Why the local store could not be opened.

use aura_store_db::seal_migration::SealMigrationError;
use aura_store_db::StoreError;
use std::path::PathBuf;

/// A failure opening the app's local store.
#[derive(Debug, thiserror::Error)]
pub enum LocalStoreError {
    /// The store is encrypted and its key is nowhere to be found.
    #[error(
        "the database at {} is encrypted, but its key is missing from {location}. \
         Restore the key there (or point AURA_STATE_KEY_FILE at a copy of it); \
         without it the data can't be read",
        db.display()
    )]
    MissingKey {
        /// The database.
        db: PathBuf,
        /// Where the key was looked for.
        location: String,
    },
    /// The key found does not open the store.
    #[error(
        "the key in {location} does not open the encrypted database at {}: \
         it was encrypted with a different key. Restore the original key \
         (or point AURA_STATE_KEY_FILE at a copy of it)",
        db.display()
    )]
    WrongKey {
        /// The database.
        db: PathBuf,
        /// Where the key came from.
        location: String,
    },
    /// Reading, creating or storing the key failed.
    #[error("database key in {location}: {detail}")]
    Key {
        /// Where the key is kept.
        location: String,
        /// What went wrong.
        detail: String,
    },
    /// Another process holds the data directory's preparation lock.
    #[error("waiting for another aura process to finish preparing {}: {detail}", path.display())]
    Locked {
        /// The lock file.
        path: PathBuf,
        /// What went wrong.
        detail: String,
    },
    /// The encrypt-in-place migration failed.
    #[error("encrypting the database: {0}")]
    Migration(SealMigrationError),
    /// RocksDB would not open the database.
    #[error("opening the database at {}: {source}", db.display())]
    Open {
        /// The database.
        db: PathBuf,
        /// The underlying failure.
        source: StoreError,
    },
}

impl LocalStoreError {
    /// True when carrying on with the unencrypted original would be
    /// wrong: the store is encrypted under a key we don't have, or the
    /// original could not be put back.
    #[must_use]
    pub const fn is_fatal(&self) -> bool {
        match self {
            Self::MissingKey { .. } | Self::WrongKey { .. } => true,
            Self::Migration(e) => e.is_fatal(),
            Self::Key { .. } | Self::Locked { .. } | Self::Open { .. } => false,
        }
    }
}
