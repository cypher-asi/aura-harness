//! The migration state machine, its verification and its marker.

use super::*;
use crate::cf;
use rocksdb::IteratorMode;

const KEY_ID: Option<&str> = Some("swarm/agents/test/state-key");
const SECRET: &[u8] = br#"{"value":"hunter2"}"#;

fn cipher() -> Arc<SealCipher> {
    Arc::new(SealCipher::new(&[5u8; 32]))
}

/// A plaintext database with one sealed-CF value and one
/// plaintext-by-design metadata value.
fn build_plaintext_db(db_path: &Path) {
    let store = RocksStore::open(db_path, false).unwrap();
    let db = store.db_handle();
    let secrets = db.cf_handle(cf::SECRETS).unwrap();
    db.put_cf(&secrets, b"api-key", SECRET).unwrap();
    let meta = db.cf_handle(cf::AGENT_META).unwrap();
    db.put_cf(&meta, b"head_seq", 7u64.to_be_bytes()).unwrap();
}

fn assert_sealed_and_readable(db_path: &Path, cipher: &Arc<SealCipher>) {
    let store = RocksStore::open_sealed(db_path, false, Some(Arc::clone(cipher))).unwrap();
    let db = store.db_handle();
    let secrets = db.cf_handle(cf::SECRETS).unwrap();
    let raw = db.get_cf(&secrets, b"api-key").unwrap().unwrap();
    assert!(
        SealCipher::is_sealed(&raw),
        "secret must be ciphertext on disk"
    );
    assert_eq!(cipher.open(&raw).unwrap(), SECRET);
    let meta = db.cf_handle(cf::AGENT_META).unwrap();
    let raw = db.get_cf(&meta, b"head_seq").unwrap().unwrap();
    assert_eq!(raw.as_slice(), 7u64.to_be_bytes());
}

fn assert_plaintext_intact(db_path: &Path) {
    let store = RocksStore::open(db_path, false).unwrap();
    let db = store.db_handle();
    let secrets = db.cf_handle(cf::SECRETS).unwrap();
    assert_eq!(db.get_cf(&secrets, b"api-key").unwrap().unwrap(), SECRET);
}

/// A plaintext database and its verified-good sealed copy.
fn copied(dir: &Path) -> (PathBuf, PathBuf, Arc<SealCipher>) {
    let src = dir.join("db");
    let dst = dir.join("copy");
    build_plaintext_db(&src);
    let cipher = cipher();
    crate::seal_db_copy(&src, &dst, &cipher).unwrap();
    (src, dst, cipher)
}

fn put_raw(path: &Path, family: &str, key: &[u8], value: &[u8]) {
    let store = RocksStore::open(path, false).unwrap();
    let db = store.db_handle();
    let handle = db.cf_handle(family).unwrap();
    db.put_cf(&handle, key, value).unwrap();
}

#[test]
fn fresh_state_needs_no_migration() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");
    std::fs::create_dir_all(&db_path).unwrap();
    let outcome = migrate_if_needed(dir.path(), &db_path, KEY_ID, &cipher()).unwrap();
    assert!(
        matches!(outcome, MigrationOutcome::NotNeeded(_)),
        "{outcome:?}"
    );
    assert!(
        !marker_path(dir.path()).exists(),
        "the caller writes a fresh marker"
    );
}

#[test]
fn plaintext_state_migrates_verifies_and_drops_the_backup() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");
    build_plaintext_db(&db_path);
    let cipher = cipher();

    let outcome = migrate_if_needed(dir.path(), &db_path, KEY_ID, &cipher).unwrap();
    let MigrationOutcome::Migrated(stats) = outcome else {
        panic!("expected a migration, got {outcome:?}");
    };
    assert_eq!((stats.values_sealed, stats.values_copied), (1, 1));
    assert!(!migrating_dir(&db_path).exists());
    assert!(
        !backup_dir(&db_path).exists(),
        "plaintext backup must be deleted"
    );
    check_key(dir.path(), &cipher).unwrap();
    assert_sealed_and_readable(&db_path, &cipher);

    let again = migrate_if_needed(dir.path(), &db_path, KEY_ID, &cipher).unwrap();
    assert!(matches!(again, MigrationOutcome::NotNeeded(_)), "{again:?}");
}

#[test]
fn every_step_rolls_back_to_the_original() {
    for at in [
        FailAt::AfterCopy,
        FailAt::AfterBackupRename,
        FailAt::AfterSwap,
        FailAt::AfterMarker,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("db");
        build_plaintext_db(&db_path);
        arm_fault(&db_path, at);

        let err = migrate_if_needed(dir.path(), &db_path, KEY_ID, &cipher()).unwrap_err();
        assert!(
            matches!(err, SealMigrationError::Interrupted(step) if step == at),
            "{err}"
        );
        assert!(!err.is_fatal());
        assert!(
            !marker_path(dir.path()).exists(),
            "{at:?}: no marker after a rollback"
        );
        assert!(!migrating_dir(&db_path).exists(), "{at:?}");
        assert!(!backup_dir(&db_path).exists(), "{at:?}");
        assert_plaintext_intact(&db_path);

        // The next start migrates.
        let outcome = migrate_if_needed(dir.path(), &db_path, KEY_ID, &cipher()).unwrap();
        assert!(matches!(outcome, MigrationOutcome::Migrated(_)), "{at:?}");
    }
}

#[test]
fn a_database_in_use_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");
    build_plaintext_db(&db_path);
    let in_use = RocksStore::open(&db_path, false).unwrap();

    let err = migrate_if_needed(dir.path(), &db_path, KEY_ID, &cipher()).unwrap_err();
    assert!(matches!(err, SealMigrationError::Store { .. }), "{err}");
    assert!(!err.is_fatal());
    drop(in_use);
    assert!(!migrating_dir(&db_path).exists());
    assert_plaintext_intact(&db_path);
}

#[test]
fn interrupted_copy_restarts_from_scratch() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");
    build_plaintext_db(&db_path);
    let temp = migrating_dir(&db_path);
    std::fs::create_dir_all(&temp).unwrap();
    std::fs::write(temp.join("000001.sst"), b"not a real sst").unwrap();

    let outcome = migrate_if_needed(dir.path(), &db_path, KEY_ID, &cipher()).unwrap();
    assert!(
        matches!(outcome, MigrationOutcome::Migrated(_)),
        "{outcome:?}"
    );
    assert!(!temp.exists());
    assert_sealed_and_readable(&db_path, &cipher());
}

#[test]
fn interrupted_swap_rolls_back_to_the_backup_and_migrates_again() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");
    build_plaintext_db(&db_path);
    std::fs::rename(&db_path, backup_dir(&db_path)).unwrap();

    let outcome = migrate_if_needed(dir.path(), &db_path, KEY_ID, &cipher()).unwrap();
    assert!(
        matches!(outcome, MigrationOutcome::Migrated(_)),
        "{outcome:?}"
    );
    assert!(!backup_dir(&db_path).exists());
    assert_sealed_and_readable(&db_path, &cipher());
}

#[test]
fn a_stale_marker_without_a_database_restores_the_backup() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");
    build_plaintext_db(&db_path);
    std::fs::rename(&db_path, backup_dir(&db_path)).unwrap();
    write_marker(dir.path(), KEY_ID, &cipher()).unwrap();

    let outcome = migrate_if_needed(dir.path(), &db_path, KEY_ID, &cipher()).unwrap();
    assert!(
        matches!(outcome, MigrationOutcome::Migrated(_)),
        "{outcome:?}"
    );
    assert_sealed_and_readable(&db_path, &cipher());
}

#[test]
fn marker_with_a_leftover_backup_finishes_the_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");
    build_plaintext_db(&db_path);
    let cipher = cipher();
    migrate_if_needed(dir.path(), &db_path, KEY_ID, &cipher).unwrap();
    let backup = backup_dir(&db_path);
    std::fs::create_dir_all(&backup).unwrap();
    std::fs::write(backup.join("CURRENT"), b"stale plaintext leftover").unwrap();

    let outcome = migrate_if_needed(dir.path(), &db_path, KEY_ID, &cipher).unwrap();
    assert!(matches!(outcome, MigrationOutcome::NotNeeded(_)));
    assert!(!backup.exists(), "deferred backup cleanup must run");
    assert_sealed_and_readable(&db_path, &cipher);
}

#[test]
fn a_wrong_key_is_refused_before_anything_changes() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");
    build_plaintext_db(&db_path);
    migrate_if_needed(dir.path(), &db_path, KEY_ID, &cipher()).unwrap();

    let wrong = Arc::new(SealCipher::new(&[6u8; 32]));
    let err = migrate_if_needed(dir.path(), &db_path, KEY_ID, &wrong).unwrap_err();
    assert!(matches!(err, SealMigrationError::WrongKey { .. }), "{err}");
    assert!(err.is_fatal());
    assert!(err.to_string().contains("different key"), "{err}");
    assert_sealed_and_readable(&db_path, &cipher());
}

#[test]
fn markers_without_a_key_check_pass_any_key() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        marker_path(dir.path()),
        br#"{"version":1,"cipher":"aes-256-gcm","key_id":"k"}"#,
    )
    .unwrap();
    check_key(dir.path(), &cipher()).unwrap();
    let marker = read_marker(dir.path()).unwrap().unwrap();
    assert_eq!(marker.key_id.as_deref(), Some("k"));
    assert!(marker.key_check.is_none());
}

#[test]
fn a_garbled_marker_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(marker_path(dir.path()), b"{not json").unwrap();
    let err = check_key(dir.path(), &cipher()).unwrap_err();
    assert!(matches!(err, SealMigrationError::Marker { .. }), "{err}");
}

#[test]
fn verify_accepts_a_faithful_copy() {
    let dir = tempfile::tempdir().unwrap();
    let (src, dst, cipher) = copied(dir.path());
    assert_eq!(verify_sealed_copy(&src, &dst, &cipher).unwrap(), 2);
}

#[test]
fn verify_rejects_a_tampered_value() {
    let dir = tempfile::tempdir().unwrap();
    let (src, dst, cipher) = copied(dir.path());
    put_raw(
        &dst,
        cf::SECRETS,
        b"api-key",
        &cipher.seal(b"other").unwrap(),
    );
    let err = verify_sealed_copy(&src, &dst, &cipher).unwrap_err();
    assert!(matches!(err, SealMigrationError::Mismatch { .. }), "{err}");
    assert!(err.to_string().contains("api-key"), "{err}");
}

#[test]
fn verify_rejects_a_missing_and_an_extra_key() {
    let dir = tempfile::tempdir().unwrap();
    let (src, dst, cipher) = copied(dir.path());
    put_raw(&src, cf::AGENT_META, b"tail_seq", &[1]);
    let err = verify_sealed_copy(&src, &dst, &cipher).unwrap_err();
    assert!(err.to_string().contains("lacks key `tail_seq`"), "{err}");

    put_raw(&dst, cf::AGENT_META, b"tail_seq", &[1]);
    put_raw(&dst, cf::CHANNELS, b"extra", &cipher.seal(b"x").unwrap());
    let err = verify_sealed_copy(&src, &dst, &cipher).unwrap_err();
    assert!(err.to_string().contains("extra key `extra`"), "{err}");
}

#[test]
fn verify_rejects_values_sealed_under_another_key() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("db");
    let foreign = SealCipher::new(&[9u8; 32]);
    put_raw(&src, cf::RECORD, b"k", &foreign.seal(b"elsewhere").unwrap());
    let cipher = cipher();
    let dst = dir.path().join("copy");
    crate::seal_db_copy(&src, &dst, &cipher).unwrap();

    let err = verify_sealed_copy(&src, &dst, &cipher).unwrap_err();
    assert!(
        matches!(err, SealMigrationError::ForeignSealedValue { .. }),
        "{err}"
    );
    assert!(err.is_fatal());

    // The whole migration refuses it the same way and keeps the source.
    let err = migrate_if_needed(dir.path(), &src, None, &cipher).unwrap_err();
    assert!(err.is_fatal(), "{err}");
    let store = RocksStore::open(&src, false).unwrap();
    let family = store.db_handle().cf_handle(cf::RECORD).unwrap();
    assert_eq!(
        store
            .db_handle()
            .iterator_cf(&family, IteratorMode::Start)
            .count(),
        1
    );
}
