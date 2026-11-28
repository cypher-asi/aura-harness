//! Opening the app's store: fresh installs, the one-time migration,
//! interruptions, and missing or wrong keys.

mod support;

use super::LocalStoreError;
use aura_store_db::seal_migration::{arm_fault, backup_dir, migrating_dir, FailAt};
use aura_store_db::{seal_db_copy, RocksStore, SealCipher};
use std::sync::Arc;
use support::*;

#[test]
fn a_fresh_install_opens_sealed() {
    let f = Fixture::new();
    let store = f.open().unwrap();
    assert!(store.cipher().is_some(), "a new store must be sealed");
    assert!(f.is_marked_sealed());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(f.key_file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the key file must be owner-only");
    }
    let written = fill(&store);
    drop(store);

    let store = f.open().unwrap();
    assert!(store.cipher().is_some());
    assert_readable(&store, &written);
    drop(store);
    assert_eq!(
        files_containing(&f.data(), SECRET),
        Vec::<std::path::PathBuf>::new()
    );
}

#[test]
fn a_plain_store_is_migrated_and_reads_back() {
    let f = Fixture::new();
    let written = f.plain_store();
    assert!(
        !files_containing(&f.data(), SECRET).is_empty(),
        "the fixture must start with plaintext on disk"
    );

    let store = f.open().unwrap();
    assert!(
        store.cipher().is_some(),
        "the migrated store must open sealed"
    );
    assert_readable(&store, &written);
    drop(store);

    assert!(f.is_marked_sealed());
    assert!(
        !backup_dir(&f.db()).exists(),
        "the plaintext copy must be removed"
    );
    assert!(!migrating_dir(&f.db()).exists());
    assert_eq!(
        files_containing(&f.data(), SECRET),
        Vec::<std::path::PathBuf>::new(),
        "no file may hold the plaintext after the migration"
    );
    let store = f.open().unwrap();
    assert_readable(&store, &written);
}

#[test]
fn an_interrupted_migration_keeps_the_original_and_retries() {
    for at in [
        FailAt::AfterCopy,
        FailAt::AfterBackupRename,
        FailAt::AfterSwap,
        FailAt::AfterMarker,
    ] {
        let f = Fixture::new();
        let written = f.plain_store();
        arm_fault(&f.db(), at);

        let store = f.open().unwrap();
        assert!(
            store.cipher().is_none(),
            "{at:?}: falls back to the original"
        );
        assert_readable(&store, &written);
        drop(store);
        assert!(!f.is_marked_sealed(), "{at:?}");
        assert!(!backup_dir(&f.db()).exists(), "{at:?}");
        assert!(!migrating_dir(&f.db()).exists(), "{at:?}");

        let store = f.open().unwrap();
        assert!(store.cipher().is_some(), "{at:?}: the next start migrates");
        assert_readable(&store, &written);
    }
}

#[test]
fn a_crash_mid_swap_recovers_from_the_backup() {
    let f = Fixture::new();
    let written = f.plain_store();
    std::fs::rename(f.db(), backup_dir(&f.db())).unwrap();

    let store = f.open().unwrap();
    assert!(store.cipher().is_some());
    assert_readable(&store, &written);
    assert!(!backup_dir(&f.db()).exists());
}

#[test]
fn a_crash_after_the_swap_before_the_marker_recovers() {
    let f = Fixture::new();
    let written = f.plain_store();
    // The sealed copy is live, the original is the backup, no marker.
    let backup = backup_dir(&f.db());
    std::fs::rename(f.db(), &backup).unwrap();
    let cipher = SealCipher::new(&[3u8; 32]);
    seal_db_copy(&backup, &f.db(), &cipher).unwrap();

    let store = f.open().unwrap();
    assert!(store.cipher().is_some());
    assert_readable(&store, &written);
}

#[test]
fn a_partial_copy_is_discarded() {
    let f = Fixture::new();
    let written = f.plain_store();
    let temp = migrating_dir(&f.db());
    std::fs::create_dir_all(&temp).unwrap();
    std::fs::write(temp.join("000009.sst"), b"half written").unwrap();

    let store = f.open().unwrap();
    assert!(store.cipher().is_some());
    assert_readable(&store, &written);
    assert!(!temp.exists());
}

/// A sealed store with data, closed, and the bytes of its database.
fn sealed_and_closed(f: &Fixture) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let store = f.open().unwrap();
    fill(&store);
    drop(store);
    snapshot(&f.db())
}

#[test]
fn a_missing_key_is_a_clear_error() {
    let f = Fixture::new();
    let before = sealed_and_closed(&f);
    std::fs::remove_file(f.key_file()).unwrap();

    let err = f.open_err();
    assert!(matches!(err, LocalStoreError::MissingKey { .. }), "{err}");
    assert!(err.is_fatal());
    let message = err.to_string();
    assert!(message.contains("key is missing"), "{message}");
    assert!(
        message.contains(&f.key_file().display().to_string()),
        "{message}"
    );
    assert!(!f.key_file().exists(), "a missing key must not be replaced");
    assert_eq!(snapshot(&f.db()), before, "the database must be untouched");
}

#[test]
fn a_wrong_key_is_a_clear_error() {
    let f = Fixture::new();
    let before = sealed_and_closed(&f);
    let other = aura_store_db::seal_key::encode_key_hex(&[9u8; 32]);
    std::fs::write(f.key_file(), other.as_bytes()).unwrap();

    let err = f.open_err();
    assert!(matches!(err, LocalStoreError::WrongKey { .. }), "{err}");
    assert!(err.is_fatal());
    assert!(err.to_string().contains("different key"), "{err}");
    assert_eq!(snapshot(&f.db()), before, "the database must be untouched");
}

#[test]
fn a_store_sealed_under_another_key_without_a_marker_is_refused() {
    let f = Fixture::new();
    let written = f.plain_store();
    let foreign = Arc::new(SealCipher::new(&[4u8; 32]));
    let backup = backup_dir(&f.db());
    std::fs::rename(f.db(), &backup).unwrap();
    seal_db_copy(&backup, &f.db(), &foreign).unwrap();
    std::fs::remove_dir_all(&backup).unwrap();

    let err = f.open_err();
    assert!(matches!(err, LocalStoreError::WrongKey { .. }), "{err}");
    let store = RocksStore::open_sealed(f.db(), false, Some(foreign)).unwrap();
    assert_readable(&store, &written);
}
