use super::*;
use tempfile::TempDir;

#[test]
fn test_tmp_path_for_appends_suffix() {
    let p = Path::new("/a/b/credentials.json");
    assert_eq!(tmp_path_for(p), Path::new("/a/b/credentials.json.tmp"));
}

#[test]
fn test_atomic_write_creates_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("creds.json");
    atomic_write_private(&path, b"hello").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
}

#[test]
fn test_atomic_write_overwrites_existing() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("creds.json");
    std::fs::write(&path, b"old contents that are longer than new").unwrap();
    atomic_write_private(&path, b"new").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
}

#[test]
fn test_atomic_write_cleans_up_tmp_on_success() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("creds.json");
    atomic_write_private(&path, b"payload").unwrap();
    let tmp = tmp_path_for(&path);
    assert!(
        !tmp.exists(),
        "tmp file should be renamed away on success, but {tmp:?} still exists"
    );
}

#[test]
fn test_atomic_write_clobbers_stale_tmp() {
    // A previous crashed write can leave <path>.tmp behind.
    // atomic_write_private must clobber it, not fail.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("creds.json");
    let tmp = tmp_path_for(&path);
    std::fs::write(&tmp, b"stale leftover from prior crash").unwrap();
    atomic_write_private(&path, b"fresh").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh");
    assert!(!tmp.exists());
}

#[test]
fn test_save_and_load_from_path_round_trip() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nested").join("credentials.json");
    save_to_path(&path, "{\"a\":1}").unwrap();
    assert_eq!(load_from_path(&path).as_deref(), Some("{\"a\":1}"));
}

#[test]
fn test_load_from_path_missing_returns_none() {
    let dir = TempDir::new().unwrap();
    assert!(load_from_path(&dir.path().join("nope.json")).is_none());
}

#[test]
fn test_read_answered_file_slot_is_absent_then_found() {
    let dir = TempDir::new().unwrap();
    let slot = SecretSlot::file(dir.path().join("key"));
    assert_eq!(slot.read_answered().unwrap(), None);
    slot.save("abc").unwrap();
    assert_eq!(slot.read_answered().unwrap().as_deref(), Some("abc"));
}

#[cfg(unix)]
#[test]
fn test_unix_perms_on_final_file_are_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("creds.json");
    atomic_write_private(&path, b"x").unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "final file should be 0600, got {mode:o}");
}

#[cfg(unix)]
#[test]
fn test_unix_perms_on_tmp_file_are_0600() {
    // We can't observe the tmp file after rename, so we stub in a
    // pre-existing tmp and check its mode right after the
    // inner open_private_for_write call by invoking it directly.
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let tmp = dir.path().join("creds.json.tmp");
    {
        let _f = open_private_for_write(&tmp).unwrap();
    }
    let mode = std::fs::metadata(&tmp).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "tmp file should be 0600, got {mode:o}");
}

// ---- sealed fallback files ------------------------------------------

const TOKEN: &str = r#"{"access_token":"sk-ant-oat-SECRET-123","refresh_token":"rt-SECRET-456"}"#;

/// A file-only slot sealed with the key in `dir/vault.key`.
fn sealed_slot(dir: &TempDir) -> SecretSlot {
    SecretSlot::file(dir.path().join("secrets").join("link.json"))
        .sealed(FileSeal::with_key_file(dir.path().join("vault.key")))
}

fn holds_plaintext(path: &Path) -> bool {
    let bytes = std::fs::read(path).unwrap();
    String::from_utf8_lossy(&bytes).contains("SECRET")
}

#[test]
fn a_sealed_slot_writes_no_plaintext_and_reads_it_back() {
    let dir = TempDir::new().unwrap();
    let slot = sealed_slot(&dir);
    slot.save(TOKEN).unwrap();
    let file = dir.path().join("secrets").join("link.json");
    assert!(
        !holds_plaintext(&file),
        "the token is not in the file's bytes"
    );
    assert!(aura_store_db::SealCipher::is_sealed(
        &std::fs::read(&file).unwrap()
    ));
    assert!(
        dir.path().join("vault.key").exists(),
        "the key was made to seal"
    );
    assert_eq!(slot.read_answered().unwrap().as_deref(), Some(TOKEN));
    assert_eq!(slot.read(), Read::Found(TOKEN.to_string()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [file, dir.path().join("vault.key")] {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{path:?}");
        }
    }
}

#[test]
fn a_plain_file_from_before_is_sealed_on_its_first_read() {
    let dir = TempDir::new().unwrap();
    let slot = sealed_slot(&dir);
    let file = dir.path().join("secrets").join("link.json");
    save_to_path(&file, TOKEN).unwrap();
    assert!(holds_plaintext(&file));
    assert_eq!(slot.read_answered().unwrap().as_deref(), Some(TOKEN));
    assert!(!holds_plaintext(&file), "migrated");
    assert!(!staged_path(&file).exists(), "no staged copy left");
    assert_eq!(slot.read_answered().unwrap().as_deref(), Some(TOKEN));
}

#[test]
fn an_interrupted_migration_keeps_the_sign_in_and_finishes_later() {
    let dir = TempDir::new().unwrap();
    let slot = sealed_slot(&dir);
    let file = dir.path().join("secrets").join("link.json");
    save_to_path(&file, TOKEN).unwrap();
    // A crash after the staged copy was written, before the rename.
    std::fs::write(staged_path(&file), b"half written").unwrap();
    assert_eq!(slot.read_answered().unwrap().as_deref(), Some(TOKEN));
    assert!(!holds_plaintext(&file));
    assert!(!staged_path(&file).exists());
}

#[test]
fn a_failed_migration_keeps_the_plain_file_and_retries() {
    let dir = TempDir::new().unwrap();
    // The key can't be made: its folder is a file.
    std::fs::write(dir.path().join("blocked"), b"").unwrap();
    let file = dir.path().join("link.json");
    let blocked = SecretSlot::file(file.clone()).sealed(FileSeal::with_key_file(
        dir.path().join("blocked").join("vault.key"),
    ));
    save_to_path(&file, TOKEN).unwrap();
    assert_eq!(blocked.read_answered().unwrap().as_deref(), Some(TOKEN));
    assert!(holds_plaintext(&file), "the plain file is kept");

    let fixed = SecretSlot::file(file.clone())
        .sealed(FileSeal::with_key_file(dir.path().join("vault.key")));
    assert_eq!(fixed.read_answered().unwrap().as_deref(), Some(TOKEN));
    assert!(!holds_plaintext(&file), "sealed on the next try");
}

#[test]
fn a_missing_or_wrong_key_is_a_clear_error_and_overwrites_nothing() {
    let dir = TempDir::new().unwrap();
    let slot = sealed_slot(&dir);
    slot.save(TOKEN).unwrap();
    let file = dir.path().join("secrets").join("link.json");
    let sealed = std::fs::read(&file).unwrap();

    std::fs::remove_file(dir.path().join("vault.key")).unwrap();
    let err = slot.read_answered().unwrap_err();
    assert!(matches!(err, AuthError::CredentialSealed { .. }), "{err}");
    let said = err.to_string();
    assert!(
        said.contains("is missing") && said.contains("vault.key"),
        "{said}"
    );
    assert!(!said.contains("SECRET"), "{said}");
    assert_eq!(slot.read(), Read::Unanswered, "unknown, not absent");
    assert!(
        !dir.path().join("vault.key").exists(),
        "no key made to read"
    );

    let other = aura_store_db::seal_key::encode_key_hex(&[7u8; 32]);
    std::fs::write(dir.path().join("vault.key"), other.as_bytes()).unwrap();
    let said = slot.read_answered().unwrap_err().to_string();
    assert!(said.contains("does not open with the key"), "{said}");
    assert_eq!(
        std::fs::read(&file).unwrap(),
        sealed,
        "the sealed file is untouched"
    );
}
