//! Reading the files work in flight changes, with real git.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use aura_exec_merge_queue::overlap::{changed_files, worktree_work};
use aura_exec_merge_queue::{EntryId, MergeEntry, MergeStatus, Origin, RepoPaths};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(60);

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str) -> String {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", file]);
    git(dir, &["rev-parse", "HEAD"])
}

fn entry(branch: &str, head: &str, remote: Option<&str>) -> MergeEntry {
    MergeEntry {
        id: EntryId::from_seq(1),
        branch: branch.to_string(),
        head: head.to_string(),
        remote: remote.map(str::to_string),
        origin: Origin::default(),
        status: MergeStatus::Queued,
        enqueued_at: chrono::Utc::now(),
        pushed_at: None,
        started_at: None,
        finished_at: None,
        updated_at: None,
        attempts: 0,
        detail: None,
        merged: None,
        files: Vec::new(),
        pr: None,
        failure: None,
    }
}

fn has_git() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

/// A repo on `main` with one commit, then `main` moved on with `m.rs`
/// after branch `aura/a` left it with `src/a.rs` and `docs/a.md`.
fn repo() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "README", "hi\n");
    git(dir.path(), &["checkout", "-q", "-b", "aura/a"]);
    commit(dir.path(), "src/a.rs", "a\n");
    let head = commit(dir.path(), "docs/a.md", "a\n");
    git(dir.path(), &["checkout", "-q", "main"]);
    commit(dir.path(), "m.rs", "m\n");
    (dir, head)
}

#[tokio::test]
async fn a_local_entry_changes_what_its_branch_added_since_it_left_the_target() {
    if !has_git() {
        return;
    }
    let (dir, head) = repo();
    let paths = RepoPaths::of(dir.path()).await.unwrap();
    let files = changed_files(&paths, "main", &entry("aura/a", &head, None), TIMEOUT)
        .await
        .unwrap();
    assert_eq!(files, ["docs/a.md", "src/a.rs"]);
}

#[tokio::test]
async fn a_remote_entry_is_read_from_the_remote_and_leaves_no_refs() {
    if !has_git() {
        return;
    }
    let (origin, head) = repo();
    let root = tempfile::tempdir().unwrap();
    let hub = root.path().join("hub");
    git(
        root.path(),
        &[
            "clone",
            "-q",
            "-b",
            "main",
            origin.path().to_str().unwrap(),
            hub.to_str().unwrap(),
        ],
    );
    // The hub's own main is behind origin's: the scan uses origin's.
    git(&hub, &["reset", "-q", "--hard", "HEAD~1"]);
    let paths = RepoPaths::of(&hub).await.unwrap();
    let files = changed_files(
        &paths,
        "main",
        &entry("aura/a", &head, Some("origin")),
        TIMEOUT,
    )
    .await
    .unwrap();
    assert_eq!(files, ["docs/a.md", "src/a.rs"]);
    let left = git(&hub, &["for-each-ref", "refs/aura-queue/"]);
    assert!(left.is_empty(), "{left}");
}

#[tokio::test]
async fn a_worktree_off_the_target_changes_its_commits_and_its_edits() {
    if !has_git() {
        return;
    }
    let (dir, _) = repo();
    assert_eq!(
        worktree_work(dir.path(), "main", TIMEOUT).await.unwrap(),
        None,
        "on the target"
    );
    git(dir.path(), &["checkout", "-q", "aura/a"]);
    std::fs::write(dir.path().join("README"), "edited\n").unwrap();
    let (branch, files) = worktree_work(dir.path(), "main", TIMEOUT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(branch, "aura/a");
    assert_eq!(files, ["README", "docs/a.md", "src/a.rs"]);
}
