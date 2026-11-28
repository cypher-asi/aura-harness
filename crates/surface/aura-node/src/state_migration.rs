//! Atomic, resumable encrypt-in-place state migration (Swarm TEE
//! upgrade R2).
//!
//! When a legacy agent's pod is recreated onto the confidential stack,
//! its first sealed boot finds **plaintext** state on the EFS subpath.
//! [`migrate_state_if_needed`] converts it to the sealed format before
//! the store is opened. The state machine (copy, full verify, swap,
//! marker, backup cleanup, rollback and crash recovery) lives in
//! [`aura_store_db::seal_migration`], shared with the local app; see its
//! module docs for the directory layout and recovery rules.
//!
//! The EFS snapshot taken before the R2 rollout remains the disaster
//! recovery path; the local backup directory only needs to survive the
//! window of a single migration.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aura_store_db::seal_migration;
use aura_store_db::SealCipher;

pub use seal_migration::{MigrationOutcome, BACKUP_SUFFIX, MIGRATING_SUFFIX};

/// `<db>.sealed-migrating` next to the database directory.
#[must_use]
pub fn migrating_dir(db_path: &Path) -> PathBuf {
    seal_migration::migrating_dir(db_path)
}

/// `<db>.plaintext-backup` next to the database directory.
#[must_use]
pub fn backup_dir(db_path: &Path) -> PathBuf {
    seal_migration::backup_dir(db_path)
}

/// Run the encrypt-in-place migration if (and only if) this sealed boot
/// found plaintext state. Called from the sealing boot flow **after**
/// the DEK is obtained and **before** the store is opened.
///
/// # Errors
///
/// Returns an error when the copy, verification or swap fails, or when
/// the DEK does not match already-sealed state. The plaintext source
/// (or its backup directory) is preserved on every error path, so a
/// failed boot can simply retry.
pub fn migrate_state_if_needed(
    data_dir: &Path,
    db_path: &Path,
    key_id: Option<&str>,
    cipher: &Arc<SealCipher>,
) -> anyhow::Result<MigrationOutcome> {
    Ok(seal_migration::migrate_if_needed(
        data_dir, db_path, key_id, cipher,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_store_db::{cf, RocksStore};

    const KEY_ID: Option<&str> = Some("swarm/agents/test/state-key");

    /// The wrapper keeps aura-node's names and behaviour: plaintext state
    /// is sealed, the backup is gone, and the next boot is a no-op.
    #[test]
    fn plaintext_state_migrates_through_the_node_wrapper() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("db");
        {
            let store = RocksStore::open(&db_path, false).unwrap();
            let db = store.db_handle();
            let secrets = db.cf_handle(cf::SECRETS).unwrap();
            db.put_cf(&secrets, b"api-key", br#"{"value":"hunter2"}"#)
                .unwrap();
        }
        let cipher = Arc::new(SealCipher::new(&[5u8; 32]));

        let first = migrate_state_if_needed(dir.path(), &db_path, KEY_ID, &cipher).unwrap();
        assert!(matches!(first, MigrationOutcome::Migrated(_)), "{first:?}");
        assert!(!migrating_dir(&db_path).exists());
        assert!(!backup_dir(&db_path).exists());
        assert!(dir
            .path()
            .join(crate::sealing::SEALED_MARKER_FILENAME)
            .exists());

        let second = migrate_state_if_needed(dir.path(), &db_path, KEY_ID, &cipher).unwrap();
        assert!(
            matches!(second, MigrationOutcome::NotNeeded(_)),
            "{second:?}"
        );

        let store = RocksStore::open_sealed(&db_path, false, Some(cipher.clone())).unwrap();
        let db = store.db_handle();
        let secrets = db.cf_handle(cf::SECRETS).unwrap();
        let raw = db.get_cf(&secrets, b"api-key").unwrap().unwrap();
        assert!(SealCipher::is_sealed(&raw));
        assert_eq!(cipher.open(&raw).unwrap(), br#"{"value":"hunter2"}"#);
    }
}
