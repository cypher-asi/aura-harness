//! Unit tests for the `git_tool` module.
//!
//! These tests exercise the internal spawn helpers against a real `git`
//! binary. If `git` is not on PATH the test is skipped (rather than
//! failing) so CI runs in environments without git (docker scratch
//! images, etc.) still pass the rest of the aura-exec-tools suite.

use super::executor::spawn_git;
use super::push::{
    check_branch_name, push_backoff_for_attempt, stderr_looks_remote_exhausted,
    stderr_looks_transient, stderr_looks_unauthorized,
};

use super::redact::redact_url;

use super::{
    git_commit_impl, git_commit_push_impl, git_push_impl, GitCommitPushTool, GitCommitTool,
    GitPushTool, GitToolError, PushPolicy, PushTarget,
};

use crate::error::ToolError;

use crate::tool::{Tool, ToolContext};

use std::path::Path;

use std::time::Duration;

use tempfile::TempDir;

use tokio::process::Command;

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

async fn init_repo(dir: &Path) {
    let status = Command::new("git")
        .arg("init")
        .current_dir(dir)
        .output()
        .await
        .expect("git init");
    assert!(status.status.success(), "git init failed: {status:?}");

    // Local identity so commits are accepted on hosts without a global config.
    for (k, v) in [
        ("user.email", "aura-test@example.com"),
        ("user.name", "Aura Test"),
    ] {
        let s = Command::new("git")
            .args(["config", k, v])
            .current_dir(dir)
            .output()
            .await
            .expect("git config");
        assert!(s.status.success(), "git config {k} failed: {s:?}");
    }
}

async fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .await
        .expect("git");
    assert!(out.status.success(), "git {args:?} failed: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repo on branch `work` with one commit, whose `origin` is a bare
/// repo next to it. Returns (parent dir, repo, bare origin).
async fn repo_with_bare_origin() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let parent = TempDir::new().unwrap();
    let origin = parent.path().join("origin.git");
    let repo = parent.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(parent.path(), &["init", "-q", "--bare", "origin.git"]).await;
    init_repo(&repo).await;
    git(&repo, &["checkout", "-q", "-b", "work"]).await;
    git(
        &repo,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    )
    .await;
    std::fs::write(repo.join("a.txt"), b"a").unwrap();
    git_commit_impl(&repo, "seed", Duration::from_secs(10))
        .await
        .expect("seed commit");
    (parent, repo, origin)
}

fn to_branch(branch: &str) -> PushTarget<'_> {
    PushTarget {
        branch: Some(branch),
        ..PushTarget::default()
    }
}

mod commit;
mod push;
mod push_policy;
