use super::*;

#[tokio::test]
async fn walk_respects_ignore_list() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::create_dir(root.join("node_modules")).unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::write(root.join("src").join("lib.rs"), "").unwrap();
    std::fs::write(root.join("node_modules").join("junk.js"), "").unwrap();

    let entries = walk_directory(root, None, 3).await;
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["src"]);
}

#[tokio::test]
async fn walk_sorts_dirs_before_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("a.txt"), "").unwrap();
    std::fs::create_dir(root.join("z_dir")).unwrap();
    let entries = walk_directory(root, None, 3).await;
    assert_eq!(entries[0].name, "z_dir");
    assert_eq!(entries[1].name, "a.txt");
}

#[tokio::test]
async fn read_capped_returns_small_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("hi.txt");
    std::fs::write(&path, "hello").unwrap();
    match read_file_capped(&path, 1024).await.unwrap() {
        ReadOutcome::Ok { bytes } => assert_eq!(bytes, b"hello"),
        ReadOutcome::TooLarge { .. } => panic!("should not be too large"),
    }
}

#[tokio::test]
async fn read_capped_rejects_oversize() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("big.bin");
    std::fs::write(&path, vec![b'A'; 100]).unwrap();
    match read_file_capped(&path, 50).await.unwrap() {
        ReadOutcome::TooLarge { max_bytes } => assert_eq!(max_bytes, 50),
        ReadOutcome::Ok { .. } => panic!("should have been too large"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn walk_breaks_symlink_loops() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let child = root.join("child");
    std::fs::create_dir(&child).unwrap();
    std::fs::write(child.join("marker.txt"), "ok").unwrap();
    std::os::unix::fs::symlink(&root, child.join("loop")).unwrap();

    let entries = walk_directory(&root, Some(&root), 20).await;

    fn count(entries: &[WalkedEntry]) -> usize {
        entries
            .iter()
            .map(|e| 1 + e.children.as_deref().map_or(0, count))
            .sum()
    }
    let n = count(&entries);
    assert!(n < 50, "symlink loop inflated tree to {n} nodes");
}

#[cfg(unix)]
#[tokio::test]
async fn walk_refuses_escaping_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_root = outside.path().canonicalize().unwrap();
    std::fs::write(outside_root.join("secret.txt"), "top-secret").unwrap();
    std::os::unix::fs::symlink(&outside_root, root.join("escape")).unwrap();

    let entries = walk_directory(&root, Some(&root), 5).await;

    fn has_name(entries: &[WalkedEntry], needle: &str) -> bool {
        entries
            .iter()
            .any(|e| e.name == needle || e.children.as_deref().is_some_and(|c| has_name(c, needle)))
    }
    assert!(!has_name(&entries, "secret.txt"));
}
