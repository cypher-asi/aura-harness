//! Which commit a project snapshot builds from, with real repositories: a
//! hub whose checkout has an unpushed commit that doesn't build, and its
//! bare `origin`, where what is shipped lives.

// Integration-test crate: test code may unwrap/expect.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use aura_engine_cloud::delta;
use aura_engine_cloud::workspace;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str) -> String {
    std::fs::write(dir.join(file), text).expect("write");
    git(dir, &["add", "-A"]);
    git(
        dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            file,
        ],
    );
    git(dir, &["rev-parse", "HEAD"])
}

struct Repos {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    hub: PathBuf,
    /// Another clone, pushing to `origin` as the merge queue does.
    queue: PathBuf,
    /// What `origin`'s main is.
    shipped: String,
}

/// `origin` with a commit on main, a hub clone with an unpushed commit on
/// top, and a second clone to move `origin` with.
fn repos() -> Repos {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let (hub, queue) = (root.join("hub"), root.join("queue"));
    git(&root, &["init", "-q", "--bare", "-b", "main", "origin.git"]);
    std::fs::create_dir(&queue).expect("mkdir");
    git(&queue, &["init", "-q", "-b", "main"]);
    git(
        &queue,
        &[
            "remote",
            "add",
            "origin",
            &root.join("origin.git").display().to_string(),
        ],
    );
    std::fs::write(queue.join("Cargo.lock"), "v1").expect("write");
    let shipped = commit(&queue, "lib.rs", "fn ok() {}");
    git(&queue, &["push", "-q", "-u", "origin", "main"]);
    git(
        &root,
        &[
            "clone",
            "-q",
            &root.join("origin.git").display().to_string(),
            "hub",
        ],
    );
    std::fs::write(hub.join("Cargo.lock"), "v2-local").expect("write");
    commit(&hub, "lib.rs", "fn broken( {}");
    Repos {
        _tmp: tmp,
        root,
        hub,
        queue,
        shipped,
    }
}

fn has_git() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

#[test]
fn a_snapshot_builds_from_origins_target_and_warns_of_the_hubs_own_commits() {
    if !has_git() {
        return;
    }
    let r = repos();
    let source = delta::build_source(&r.hub, "main").expect("source");
    assert_eq!(source.commit, r.shipped, "not the hub's unpushed commit");
    assert_eq!(source.upstream.as_deref(), Some("refs/remotes/origin/main"));
    let note = source.note.clone().expect("a warning");
    assert!(note.contains("1 commit(s) not on origin/main"), "{note}");
    assert_eq!(
        delta::file_at(&r.hub, &source.commit, "Cargo.lock").as_deref(),
        Some(&b"v1"[..]),
        "the lockfile is the shipped one"
    );

    // The builder gets that commit, without the hub's.
    let history = delta::bundle_source(&r.hub, &source).expect("bundle");
    std::fs::write(r.root.join("aura-project.bundle"), &history.bytes).expect("write");
    let machine = r.root.join("machine");
    let script = workspace::clone_snapshot(&history.commit, history.upstream.as_deref())
        .expect("script")
        .replace("/tmp/", &format!("{}/", r.root.display()));
    let out = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .env("AURA_WORKSPACE", &machine)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("sh");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(git(&machine, &["rev-parse", "HEAD"]), r.shipped);
    assert_eq!(
        std::fs::read_to_string(machine.join("lib.rs")).unwrap(),
        "fn ok() {}"
    );
}

#[test]
fn a_snapshot_follows_origin_as_the_queue_moves_it() {
    if !has_git() {
        return;
    }
    let r = repos();
    let landed = commit(&r.queue, "next.rs", "fn next() {}");
    git(&r.queue, &["push", "-q", "origin", "main"]);
    let source = delta::build_source(&r.hub, "main").expect("source");
    assert_eq!(source.commit, landed, "fetched before building");
}

#[test]
fn without_origin_a_snapshot_builds_from_head_and_says_so() {
    if !has_git() {
        return;
    }
    let r = repos();
    git(&r.hub, &["remote", "remove", "origin"]);
    let head = git(&r.hub, &["rev-parse", "HEAD"]);
    let source = delta::build_source(&r.hub, "main").expect("source");
    assert_eq!(
        (source.commit.as_str(), source.upstream),
        (head.as_str(), None)
    );
    let note = source.note.expect("a warning");
    assert!(note.contains("building from the hub's HEAD"), "{note}");
}
