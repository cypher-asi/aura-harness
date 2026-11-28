#![allow(clippy::unwrap_used)]

use super::*;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repo on `branch` with one commit, holding `config` when given.
fn repo(branch: &str, config: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", branch]);
    std::fs::write(dir.path().join("README"), "x").unwrap();
    if let Some(config) = config {
        write_config(dir.path(), config);
    }
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-qm", "seed"]);
    dir
}

fn write_config(dir: &Path, text: &str) {
    std::fs::create_dir_all(dir.join(".aura")).unwrap();
    std::fs::write(dir.join(CONFIG_PATH), text).unwrap();
}

#[tokio::test]
async fn the_config_committed_on_main_names_the_target() {
    let dir = repo("main", Some("target = \"trunk\"\n"));
    assert_eq!(resolve_target(dir.path()).await, "trunk");
}

#[tokio::test]
async fn main_without_a_config_merges_into_main() {
    let dir = repo("main", None);
    assert_eq!(resolve_target(dir.path()).await, "main");
}

#[tokio::test]
async fn an_uncommitted_config_does_not_move_the_target() {
    let dir = repo("main", Some("target = \"trunk\"\n"));
    write_config(dir.path(), "target = \"elsewhere\"\n");
    assert_eq!(resolve_target(dir.path()).await, "trunk");
    let bare = repo("main", None);
    write_config(bare.path(), "target = \"elsewhere\"\n");
    assert_eq!(resolve_target(bare.path()).await, "main");
}

#[tokio::test]
async fn only_origin_main_is_read_when_there_is_no_local_main() {
    let origin = repo("main", Some("target = \"trunk\"\n"));
    let root = tempfile::tempdir().unwrap();
    let clone = root.path().join("clone");
    let url = origin.path().display().to_string();
    git(root.path(), &["clone", "-q", &url, "clone"]);
    git(&clone, &["checkout", "-q", "-b", "aura/work"]);
    git(&clone, &["branch", "-q", "-D", "main"]);
    assert_eq!(resolve_target(&clone).await, "trunk");
}

#[tokio::test]
async fn a_repo_without_main_reads_its_working_tree() {
    let dir = repo("trunk", Some("target = \"trunk\"\n"));
    assert_eq!(resolve_target(dir.path()).await, "trunk");
    let plain = repo("dev", None);
    assert_eq!(resolve_target(plain.path()).await, "main");
}

#[tokio::test]
async fn a_folder_that_is_no_repo_still_has_a_target() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(resolve_target(dir.path()).await, "main");
    write_config(dir.path(), "target = \"trunk\"\n");
    assert_eq!(resolve_target(dir.path()).await, "trunk");
}
