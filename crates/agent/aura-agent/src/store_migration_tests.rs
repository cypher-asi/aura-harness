use crate::session_bootstrap::resolve_store_path;
use std::fs;
use tempfile::TempDir;

#[test]
fn fresh_install_returns_canonical() {
    let tmp = TempDir::new().unwrap();
    let result = resolve_store_path(tmp.path());
    assert_eq!(result, tmp.path().join("db"));
}

#[test]
fn legacy_migrated_to_canonical() {
    let tmp = TempDir::new().unwrap();
    let legacy = tmp.path().join("store");
    fs::create_dir(&legacy).unwrap();
    fs::write(legacy.join("data.txt"), "test").unwrap();

    let result = resolve_store_path(tmp.path());
    assert_eq!(result, tmp.path().join("db"));
    assert!(tmp.path().join("db").exists());
    assert!(!legacy.exists());
    assert!(tmp.path().join("db").join("data.txt").exists());
}

#[test]
fn canonical_exists_removes_legacy() {
    let tmp = TempDir::new().unwrap();
    let canonical = tmp.path().join("db");
    let legacy = tmp.path().join("store");
    fs::create_dir(&canonical).unwrap();
    fs::create_dir(&legacy).unwrap();

    let result = resolve_store_path(tmp.path());
    assert_eq!(result, canonical);
    assert!(!legacy.exists(), "legacy directory should be auto-removed");
}

#[test]
fn canonical_already_exists_returns_canonical() {
    let tmp = TempDir::new().unwrap();
    let canonical = tmp.path().join("db");
    fs::create_dir(&canonical).unwrap();

    let result = resolve_store_path(tmp.path());
    assert_eq!(result, canonical);
}

#[test]
fn both_exist_removes_legacy_with_contents() {
    let tmp = TempDir::new().unwrap();
    let canonical = tmp.path().join("db");
    let legacy = tmp.path().join("store");
    fs::create_dir(&canonical).unwrap();
    fs::create_dir(&legacy).unwrap();
    fs::write(legacy.join("stale.txt"), "old data").unwrap();

    let result = resolve_store_path(tmp.path());
    assert_eq!(result, canonical);
    assert!(
        !legacy.exists(),
        "legacy directory should be auto-removed even with contents"
    );
}
