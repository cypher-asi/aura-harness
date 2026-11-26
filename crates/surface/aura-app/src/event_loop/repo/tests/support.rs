//! Helpers shared by the tests that read a real repo.

use super::super::load;
use aura_core_types::AgentId;
use aura_ui::repo::{EventKind, RepoEvent, RepoView};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// `git <args>` in `dir` at Unix time `at`, with no config but the repo's.
fn git_at(dir: &Path, at: i64, args: &[&str]) -> String {
    let date = format!("@{at} +0000");
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .env("GIT_AUTHOR_NAME", "A")
        .env("GIT_AUTHOR_EMAIL", "a@example.com")
        .env("GIT_COMMITTER_NAME", "A")
        .env("GIT_COMMITTER_EMAIL", "a@example.com")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn commit(dir: &Path, at: i64, file: &str, text: &str, subject: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git_at(dir, at, &["add", file]);
    git_at(dir, at, &["commit", "-q", "-m", subject]);
}

pub(super) fn find<'a>(view: &'a RepoView, kind: EventKind, title: &str) -> &'a RepoEvent {
    view.events
        .iter()
        .find(|e| e.kind == kind && e.title == title)
        .unwrap_or_else(|| panic!("no {kind:?} {title:?} in {:#?}", view.events))
}

/// The agent whose REPO tab [`viewed`] reads.
pub(super) fn agent() -> AgentId {
    AgentId::new([3; 32])
}

/// A repo `work` in `dir`, beside the bare `origin` it pushes to: First
/// and Second pushed, Third not yet. Returns `work`.
fn pushed(dir: &Path) -> PathBuf {
    let (origin, work) = (dir.join("origin.git"), dir.join("work"));
    std::fs::create_dir_all(&work).unwrap();
    git_at(
        dir,
        1000,
        &["init", "-q", "--bare", "-b", "main", "origin.git"],
    );
    git_at(&work, 1000, &["init", "-q", "-b", "main"]);
    git_at(
        &work,
        1000,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    commit(&work, 1001, "a.txt", "one\n", "First");
    git_at(&work, 1002, &["push", "-q", "-u", "origin", "main"]);
    commit(&work, 1003, "a.txt", "two\n", "Second");
    git_at(&work, 1004, &["push", "-q"]);
    commit(&work, 1005, "a.txt", "three\n", "Third");
    work
}

/// Go on with the history of `work`, the repo [`pushed`] made in `dir`:
/// `feature` merged in, a commit reset away, a stash, a worktree on a
/// new branch `side`, and changes not committed.
fn branched(dir: &Path, work: &Path) {
    git_at(work, 1006, &["checkout", "-q", "-b", "feature"]);
    commit(work, 1007, "b.txt", "feature\n", "Feature");
    git_at(work, 1008, &["checkout", "-q", "main"]);
    git_at(
        work,
        1009,
        &["merge", "-q", "--no-ff", "feature", "-m", "Merge feature"],
    );
    commit(work, 1010, "a.txt", "oops\n", "Oops");
    git_at(work, 1011, &["reset", "-q", "--hard", "HEAD~1"]);
    std::fs::write(work.join("a.txt"), "stashed\n").unwrap();
    git_at(work, 1012, &["stash", "push", "-q", "-m", "Try it"]);
    let linked = dir.join("linked");
    git_at(
        work,
        1013,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            linked.to_str().unwrap(),
        ],
    );
    std::fs::write(work.join("a.txt"), "changed\n").unwrap();
    std::fs::write(work.join("new.txt"), "new\n").unwrap();
}

/// A repo with a history to show ([`pushed`], then [`branched`]), read
/// into the REPO tab of [`agent`]: the folder it is in, its work tree
/// and the view.
pub(super) async fn viewed() -> (TempDir, PathBuf, RepoView) {
    let dir = tempfile::tempdir().unwrap();
    let work = pushed(dir.path());
    branched(dir.path(), &work);
    let (view, reading) = load::view(&work, agent(), false).await;
    assert!(reading.is_none(), "no GitHub, no PRs to read");
    (dir, work, view)
}
