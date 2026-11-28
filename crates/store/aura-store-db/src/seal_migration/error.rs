//! Errors of the encrypt-in-place migration.

use std::path::PathBuf;

use crate::error::StoreError;

/// Why preparing or migrating a sealed store failed.
///
/// Every variant names the step and the path involved. See
/// [`SealMigrationError::is_fatal`] for which ones may be answered by
/// carrying on with the original plaintext data.
#[derive(Debug, thiserror::Error)]
pub enum SealMigrationError {
    /// A filesystem step failed.
    #[error("{step} ({}): {source}", path.display())]
    Io {
        /// What was being done.
        step: &'static str,
        /// The path it was done to.
        path: PathBuf,
        /// The underlying failure.
        source: std::io::Error,
    },
    /// A database step failed.
    #[error("{step} ({}): {source}", path.display())]
    Store {
        /// What was being done.
        step: &'static str,
        /// The database involved.
        path: PathBuf,
        /// The underlying failure.
        source: StoreError,
    },
    /// The sealed copy does not match its source.
    #[error("sealed copy {} does not match its source: {detail}", path.display())]
    Mismatch {
        /// The sealed copy.
        path: PathBuf,
        /// What differs: the column family, and the key when there is one.
        detail: String,
    },
    /// The source already holds sealed values this key can't open: it
    /// was sealed under another key.
    #[error(
        "database {} holds values sealed under a different key \
         (column family `{column_family}`); refusing to migrate it with this key",
        path.display()
    )]
    ForeignSealedValue {
        /// The source database.
        path: PathBuf,
        /// Where the first such value was found.
        column_family: String,
    },
    /// The `.aura-sealed` marker could not be read or parsed.
    #[error("sealed marker {}: {detail}", path.display())]
    Marker {
        /// The marker file.
        path: PathBuf,
        /// What is wrong with it.
        detail: String,
    },
    /// The key does not open the marker's key check: the store was
    /// sealed under another key.
    #[error(
        "the key does not match the sealed store (key check in {} failed); \
         the store was sealed under a different key",
        marker.display()
    )]
    WrongKey {
        /// The marker holding the key check.
        marker: PathBuf,
    },
    /// A step failed and putting the original database back failed too.
    /// The plaintext is still in `backup`.
    #[error(
        "{cause}; restoring the original database from {} also failed: {source}",
        backup.display()
    )]
    RollbackFailed {
        /// The failure that started the rollback.
        cause: Box<SealMigrationError>,
        /// Where the original plaintext database is.
        backup: PathBuf,
        /// Why restoring it failed.
        source: std::io::Error,
    },
    /// A test stopped the migration at this step.
    #[error("migration interrupted at {0:?} (test fault)")]
    Interrupted(super::FailAt),
}

impl SealMigrationError {
    /// True when the caller must not fall back to the original plaintext
    /// data: the key is wrong for data that is already sealed, or the
    /// original could not be put back. Every other failure leaves the
    /// original database where it was, safe to use and to migrate again
    /// on the next start.
    #[must_use]
    pub const fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::WrongKey { .. } | Self::ForeignSealedValue { .. } | Self::RollbackFailed { .. }
        )
    }

    pub(crate) fn io(step: &'static str, path: &std::path::Path, source: std::io::Error) -> Self {
        Self::Io {
            step,
            path: path.to_path_buf(),
            source,
        }
    }

    pub(crate) fn store(step: &'static str, path: &std::path::Path, source: StoreError) -> Self {
        Self::Store {
            step,
            path: path.to_path_buf(),
            source,
        }
    }

    pub(crate) fn mismatch(path: &std::path::Path, detail: String) -> Self {
        Self::Mismatch {
            path: path.to_path_buf(),
            detail,
        }
    }
}
