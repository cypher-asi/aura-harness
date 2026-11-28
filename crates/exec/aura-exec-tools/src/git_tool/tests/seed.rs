//! A cloud machine seeded with the hub's uncommitted work never commits it.

use super::*;
use crate::git_tool::git_commit_impl;

const T: Duration = Duration::from_secs(20);

/// A repo with `a.txt` committed, then seeded as old machines were: the
/// hub's edit of `a.txt` and its new `wip.txt` unpacked uncommitted, and
/// that starting point recorded at `refs/aura/base`.
async fn seeded_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    init_repo(dir.path()).await;
    std::fs::write(dir.path().join("a.txt"), "first\n").unwrap();
    git_commit_impl(dir.path(), "first", T).await.unwrap();
    std::fs::write(dir.path().join("a.txt"), "hub's half-done edit\n").unwrap();
    std::fs::write(dir.path().join("wip.txt"), "hub's work in progress\n").unwrap();
    // As the hub's `mark_base` does, with a scratch index.
    let idx = dir.path().join(".git/aura-base.index");
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .env("GIT_INDEX_FILE", &idx)
            .env("GIT_AUTHOR_NAME", "aura")
            .env("GIT_AUTHOR_EMAIL", "aura@localhost")
            .env("GIT_COMMITTER_NAME", "aura")
            .env("GIT_COMMITTER_EMAIL", "aura@localhost")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    run(&["read-tree", "HEAD"]);
    run(&["add", "-A"]);
    let tree = run(&["write-tree"]);
    let base = run(&[
        "commit-tree",
        &tree,
        "-p",
        "HEAD",
        "-m",
        "aura: hub working tree",
    ]);
    run(&["update-ref", "refs/aura/base", &base]);
    dir
}

async fn committed_files(dir: &Path) -> String {
    git(dir, &["show", "--name-only", "--format=", "HEAD"]).await
}

#[tokio::test]
async fn a_commit_leaves_the_seeded_hub_changes_out() {
    if !git_available() {
        return;
    }
    let dir = seeded_repo().await;
    std::fs::write(dir.path().join("mine.txt"), "the agent's work\n").unwrap();
    let sha = git_commit_impl(dir.path(), "agent work", T).await.unwrap();
    assert!(sha.is_some());
    assert_eq!(committed_files(dir.path()).await, "mine.txt");
    // The hub's work is still there, uncommitted.
    let left = git(dir.path(), &["status", "--porcelain"]).await;
    assert!(
        left.contains("M a.txt") && left.contains("?? wip.txt"),
        "{left}"
    );
}

#[tokio::test]
async fn only_seeded_changes_means_nothing_to_commit() {
    if !git_available() {
        return;
    }
    let dir = seeded_repo().await;
    assert_eq!(
        git_commit_impl(dir.path(), "nothing", T).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn editing_a_seeded_file_refuses_the_commit() {
    if !git_available() {
        return;
    }
    let dir = seeded_repo().await;
    let before = git(dir.path(), &["rev-parse", "HEAD"]).await;
    std::fs::write(dir.path().join("a.txt"), "hub's edit, then the agent's\n").unwrap();
    std::fs::write(dir.path().join("mine.txt"), "the agent's work\n").unwrap();
    let err = git_commit_impl(dir.path(), "mixed", T).await.unwrap_err();
    match &err {
        GitToolError::SeededHubChanges { files } => assert_eq!(files, &["a.txt".to_string()]),
        other => panic!("expected SeededHubChanges, got {other:?}"),
    }
    assert_eq!(
        git(dir.path(), &["rev-parse", "HEAD"]).await,
        before,
        "nothing committed"
    );
}

#[tokio::test]
async fn a_machine_placed_since_commits_as_before() {
    if !git_available() {
        return;
    }
    let dir = TempDir::new().unwrap();
    init_repo(dir.path()).await;
    std::fs::write(dir.path().join("a.txt"), "x\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "y\n").unwrap();
    git_commit_impl(dir.path(), "both", T).await.unwrap();
    let files = committed_files(dir.path()).await;
    assert!(
        files.contains("a.txt") && files.contains("b.txt"),
        "{files}"
    );
}
