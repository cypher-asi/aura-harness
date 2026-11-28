//! The pull run against real repos: a bare `origin` standing in for
//! GitHub, and a machine's workspace behind it.

use super::*;
use crate::workspace::{fork_branch, start_branch, BranchStart, BASE_REF};
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
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
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", file]);
    git(dir, &["rev-parse", "HEAD"])
}

/// GitHub's `origin` with `main`, a machine cloned from it on the agent's
/// branch, and a second clone that moves `main` on.
struct Repos {
    dir: tempfile::TempDir,
    machine: PathBuf,
    other: PathBuf,
}

fn repos() -> Repos {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    let seed = dir.path().join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    let origin_arg = origin.display().to_string();
    git(
        dir.path(),
        &["init", "-q", "--bare", "-b", "main", &origin_arg],
    );
    git(&seed, &["init", "-q", "-b", "main"]);
    commit(&seed, "README", "hello\n");
    git(&seed, &["remote", "add", "origin", &origin_arg]);
    git(&seed, &["push", "-q", "origin", "main"]);
    let machine = dir.path().join("machine");
    let other = dir.path().join("other");
    for clone in [&machine, &other] {
        git(
            dir.path(),
            &["clone", "-q", &origin_arg, &clone.display().to_string()],
        );
    }
    git(&machine, &["checkout", "-q", "-b", "aura/scout-sc01"]);
    Repos {
        dir,
        machine,
        other,
    }
}

impl Repos {
    /// Another agent's work lands on `origin`'s main.
    fn main_moves(&self, file: &str, text: &str) -> String {
        let sha = commit(&self.other, file, text);
        git(&self.other, &["push", "-q", "origin", "main"]);
        sha
    }

    /// Run `script` in the machine's workspace as its exec would, with
    /// `/tmp/` kept to this test. Returns the pull's answer.
    fn run(&self, script: &str) -> String {
        let tmp = self.dir.path().join("tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let out = Command::new("sh")
            .arg("-c")
            .arg(script.replace("/tmp/", &format!("{}/", tmp.display())))
            .env("AURA_WORKSPACE", &self.machine)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(
            out.status.success(),
            "{stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        parse_pulled(&stdout).unwrap_or_default().to_string()
    }

    fn pull(&self) -> String {
        self.run(&format!(
            "set -e; cd \"$AURA_WORKSPACE\"; {}",
            pull_latest(PULL_BRANCH).unwrap()
        ))
    }

    fn head(&self) -> String {
        git(&self.machine, &["rev-parse", "HEAD"])
    }

    fn has_merge_in_progress(&self) -> bool {
        self.machine.join(".git/MERGE_HEAD").exists()
    }
}

fn has_git() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

#[test]
fn a_machine_behind_github_merges_the_latest_main() {
    if !has_git() {
        return;
    }
    let repos = repos();
    commit(&repos.machine, "mine.txt", "agent\n");
    let landed = repos.main_moves("theirs.txt", "landed by the queue\n");
    let said = repos.pull();
    assert!(said.starts_with("merged origin/main"), "{said}");
    assert!(repos.machine.join("theirs.txt").exists());
    assert!(repos.machine.join("mine.txt").exists());
    git(
        &repos.machine,
        &["merge-base", "--is-ancestor", &landed, "HEAD"],
    );
    assert_eq!(
        git(&repos.machine, &["branch", "--show-current"]),
        "aura/scout-sc01"
    );
}

#[test]
fn a_machine_on_the_latest_main_is_up_to_date() {
    if !has_git() {
        return;
    }
    let repos = repos();
    let before = repos.head();
    assert_eq!(repos.pull(), "up to date with origin/main");
    assert_eq!(repos.head(), before);
}

#[test]
fn a_clean_branch_behind_main_fast_forwards() {
    if !has_git() {
        return;
    }
    let repos = repos();
    let landed = repos.main_moves("theirs.txt", "x\n");
    assert!(repos.pull().starts_with("merged"));
    assert_eq!(repos.head(), landed, "no merge commit when none is needed");
}

#[test]
fn without_origin_or_its_main_the_pull_is_skipped() {
    if !has_git() {
        return;
    }
    let repos = repos();
    git(
        &repos.machine,
        &["remote", "set-url", "origin", "/nowhere/at/all"],
    );
    let said = repos.pull();
    assert!(
        said.starts_with("skipped: could not fetch origin/main"),
        "{said}"
    );
    git(&repos.machine, &["remote", "remove", "origin"]);
    assert_eq!(repos.pull(), "skipped: no origin");
}

#[test]
fn a_conflicting_main_is_left_out_and_the_branch_kept() {
    if !has_git() {
        return;
    }
    let repos = repos();
    let mine = commit(&repos.machine, "README", "mine\n");
    repos.main_moves("README", "theirs\n");
    let said = repos.pull();
    assert!(
        said.starts_with("skipped: origin/main does not merge cleanly"),
        "{said}"
    );
    assert_eq!(repos.head(), mine);
    assert!(!repos.has_merge_in_progress());
    assert_eq!(git(&repos.machine, &["status", "--porcelain"]), "");
}

#[test]
fn uncommitted_changes_main_touches_are_kept_and_main_left_out() {
    if !has_git() {
        return;
    }
    let repos = repos();
    let before = repos.head();
    std::fs::write(repos.machine.join("README"), "hub's uncommitted edit\n").unwrap();
    repos.main_moves("README", "theirs\n");
    let said = repos.pull();
    assert!(
        said.starts_with("skipped: origin/main does not merge cleanly"),
        "{said}"
    );
    assert_eq!(repos.head(), before);
    let kept = std::fs::read_to_string(repos.machine.join("README")).unwrap();
    assert_eq!(kept, "hub's uncommitted edit\n");
}

#[test]
fn uncommitted_changes_elsewhere_stay_on_top_of_the_merge() {
    if !has_git() {
        return;
    }
    let repos = repos();
    std::fs::write(repos.machine.join("README"), "hub's uncommitted edit\n").unwrap();
    repos.main_moves("theirs.txt", "x\n");
    assert!(repos.pull().starts_with("merged"));
    assert!(repos.machine.join("theirs.txt").exists());
    let kept = std::fs::read_to_string(repos.machine.join("README")).unwrap();
    assert_eq!(kept, "hub's uncommitted edit\n", "still uncommitted");
    assert_eq!(git(&repos.machine, &["status", "--porcelain"]), "M README");
}

#[test]
fn a_fork_pulls_the_latest_main_onto_its_new_branch() {
    if !has_git() {
        return;
    }
    let repos = repos();
    repos.main_moves("theirs.txt", "x\n");
    let said = repos.run(&fork_branch("aura/child-ch01", "main").unwrap());
    assert!(said.starts_with("merged origin/main"), "{said}");
    assert_eq!(
        git(&repos.machine, &["branch", "--show-current"]),
        "aura/child-ch01"
    );
    assert!(repos.machine.join("theirs.txt").exists());
}

#[test]
fn a_new_machine_pulls_after_the_hubs_changes_and_before_marking_its_base() {
    let sha = "e91dfbe90eb7b51f7b0459ce0c829d439a150396";
    let start = BranchStart {
        branch: "aura/a-b",
        head: sha,
        target: "main",
        remote_url: Some("https://github.com/a/b.git"),
        bundle: false,
        delta: true,
    };
    let script = start_branch(&start).unwrap();
    let pull = script.find("git fetch -q --no-tags origin").unwrap();
    assert!(script.find("aura-delta.tgz").unwrap() < pull);
    assert!(
        pull < script.find(BASE_REF).unwrap(),
        "the base is the pulled tree"
    );
    let without_origin = start_branch(&BranchStart {
        remote_url: None,
        ..start
    })
    .unwrap();
    assert!(!without_origin.contains(PULLED), "nothing to pull from");
}

#[test]
fn a_fork_pulls_the_queue_target_not_main() {
    if !has_git() {
        return;
    }
    let repos = repos();
    commit(&repos.other, "trunk.txt", "t\n");
    git(
        &repos.other,
        &["push", "-q", "origin", "HEAD:refs/heads/trunk"],
    );
    repos.main_moves("main.txt", "m\n");
    let said = repos.run(&fork_branch("aura/child-ch01", "trunk").unwrap());
    assert!(said.starts_with("merged origin/trunk"), "{said}");
    assert!(repos.machine.join("trunk.txt").exists());
    assert!(
        !repos.machine.join("main.txt").exists(),
        "main is not pulled"
    );
}

#[test]
fn a_new_machine_fetches_its_queue_target() {
    let start = BranchStart {
        branch: "aura/a-b",
        head: "e91dfbe90eb7b51f7b0459ce0c829d439a150396",
        target: "trunk",
        remote_url: Some("https://github.com/a/b.git"),
        bundle: false,
        delta: false,
    };
    let script = start_branch(&start).unwrap();
    assert!(
        script.contains("+refs/heads/trunk:refs/remotes/origin/trunk"),
        "{script}"
    );
    assert!(!script.contains("refs/heads/main"), "{script}");
    let evil = BranchStart {
        target: "a'b",
        ..start
    };
    assert!(start_branch(&evil).is_err());
}

#[test]
fn only_a_plain_branch_is_pulled() {
    for evil in ["", "a'b", "$(id)", "-x"] {
        assert!(pull_latest(evil).is_err(), "{evil:?}");
    }
    assert_eq!(
        parse_pulled("noise\naura-pull: up to date\n"),
        Some("up to date")
    );
    assert_eq!(parse_pulled("nothing"), None);
}
