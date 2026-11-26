//! A project's trip to a cloud machine and back, with real repositories:
//! a "hub" repository (with a bare `origin`), and a "machine" workspace
//! made from its bundles by the scripts machines run.

use std::path::{Path, PathBuf};
use std::process::Command;

use aura_engine_cloud::delta::{self, BranchFetch};
use aura_engine_cloud::workspace::{self, BranchStart, Bundled, BASE_REF};

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

/// Run a machine script with `/tmp/` pointing at `root`, where the test
/// puts the "uploads".
fn sh(root: &Path, workspace: &Path, script: &str) -> String {
    let out = Command::new("sh")
        .arg("-c")
        .arg(script.replace("/tmp/", &format!("{}/", root.display())))
        .env("AURA_WORKSPACE", workspace)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("sh");
    assert!(
        out.status.success(),
        "{script}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).expect("write");
}

fn commit(dir: &Path, file: &str, text: &str, message: &str) -> String {
    write(&dir.join(file), text);
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
            message,
        ],
    );
    git(dir, &["rev-parse", "HEAD"])
}

/// The working tree as a tree object, the way the node's patch takes it:
/// from a scratch index.
fn worktree(root: &Path, dir: &Path) -> String {
    sh(
        root,
        dir,
        "set -e; cd \"$AURA_WORKSPACE\"; cp .git/index .git/t.index; \
         GIT_INDEX_FILE=.git/t.index git add -A; GIT_INDEX_FILE=.git/t.index git write-tree; \
         rm .git/t.index",
    )
    .trim()
    .to_string()
}

struct Setup {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    hub: PathBuf,
    machine: PathBuf,
}

/// A hub repository with two commits pushed to a bare `origin`.
fn setup() -> Setup {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let hub = root.join("hub");
    let origin = root.join("origin.git");
    std::fs::create_dir(&hub).expect("mkdir");
    git(&root, &["init", "-q", "--bare", "-b", "main", "origin.git"]);
    git(&hub, &["init", "-q", "-b", "main"]);
    git(&hub, &["config", "user.name", "t"]);
    git(&hub, &["config", "user.email", "t@t"]);
    git(
        &hub,
        &["remote", "add", "origin", &origin.display().to_string()],
    );
    commit(&hub, "a.rs", "one", "first");
    commit(&hub, "b.rs", "two", "second");
    git(&hub, &["push", "-q", "-u", "origin", "main"]);
    Setup {
        _tmp: tmp,
        machine: root.join("machine"),
        root,
        hub,
    }
}

/// Build the machine the way a snapshot build does.
fn snapshot(s: &Setup) -> String {
    let history = delta::bundle_head(&s.hub).expect("bundle");
    assert_eq!(
        history.upstream.as_deref(),
        Some("refs/remotes/origin/main")
    );
    write_bytes(&s.root.join("aura-project.bundle"), &history.bytes);
    sh(
        &s.root,
        &s.machine,
        &workspace::clone_snapshot(&history.commit, history.upstream.as_deref()).expect("script"),
    );
    history.commit
}

fn write_bytes(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).expect("write");
}

/// Place an agent on `branch` the way the hub does: newer commits,
/// uncommitted changes, then the branch script.
fn place(s: &Setup, snapshot: &str, branch: &str) -> String {
    let head = delta::head_commit(&s.hub).expect("head");
    let newer = delta::bundle_since(&s.hub, snapshot).expect("since");
    if let Some(bytes) = &newer {
        write_bytes(&s.root.join("aura-head.bundle"), bytes);
    }
    let changes = delta::collect(&s.hub, &head).expect("delta");
    if !changes.is_empty() {
        write_bytes(
            &s.root.join("aura-delta.tgz"),
            &delta::pack(&s.hub, &changes).expect("pack"),
        );
    }
    let start = BranchStart {
        branch,
        head: &head,
        remote_url: Some("https://github.com/acme/app.git"),
        bundle: newer.is_some(),
        delta: !changes.is_empty(),
    };
    sh(
        &s.root,
        &s.machine,
        &workspace::start_branch(&start).expect("script"),
    );
    head
}

fn bring_back(s: &Setup, branch: &str, fetched: Option<&str>) -> Option<BranchFetch> {
    let out = sh(
        &s.root,
        &s.machine,
        &workspace::bundle_branch(branch, fetched).expect("script"),
    );
    match workspace::parse_bundled(&out).expect("answer") {
        Bundled::Nothing => None,
        Bundled::Commits { .. } => Some(
            delta::fetch_branch(&s.hub, &s.root.join("aura-out.bundle"), branch).expect("fetch"),
        ),
    }
}

#[test]
fn a_machine_gets_history_a_branch_and_a_base_that_hides_the_hubs_changes() {
    let s = setup();
    let first = git(&s.hub, &["rev-list", "--max-parents=0", "HEAD"]);
    let snap = snapshot(&s);
    assert_eq!(git(&s.machine, &["rev-parse", "HEAD"]), snap);
    assert!(
        git(&s.machine, &["log", "--format=%H"]).contains(&first),
        "the whole history"
    );
    assert_eq!(git(&s.machine, &["remote"]), "", "no origin in a snapshot");
    assert_eq!(
        git(&s.machine, &["rev-parse", "refs/remotes/origin/main"]),
        snap,
        "what origin has travels along"
    );

    // The hub moves on: an unpushed commit, and uncommitted work.
    let head = commit(&s.hub, "c.rs", "three", "third");
    write(&s.hub.join("a.rs"), "one, edited");
    write(&s.hub.join("new.rs"), "untracked");
    write(&s.hub.join(".env"), "SECRET=1");
    assert_eq!(place(&s, &snap, "aura/reviewer-re36"), head);

    let m = &s.machine;
    assert_eq!(git(m, &["branch", "--show-current"]), "aura/reviewer-re36");
    assert_eq!(
        git(m, &["rev-parse", "HEAD"]),
        head,
        "unpushed commits arrive"
    );
    assert_eq!(
        git(m, &["remote", "get-url", "origin"]),
        "https://github.com/acme/app.git"
    );
    assert_eq!(
        git(
            m,
            &["log", "--format=%s", "HEAD", "--not", "--remotes=origin"]
        ),
        "third",
        "pushes know what origin already has"
    );
    assert_eq!(
        std::fs::read_to_string(m.join("a.rs")).expect("read"),
        "one, edited"
    );
    assert!(!m.join(".env").exists(), ".env never travels");
    let status = git(m, &["status", "--porcelain"]);
    let lines: Vec<&str> = status.lines().map(str::trim).collect();
    assert_eq!(
        lines,
        ["M a.rs", "?? new.rs"],
        "the hub's changes stay uncommitted"
    );
    assert_eq!(
        git(m, &["rev-parse", &format!("{BASE_REF}^{{tree}}")]),
        worktree(&s.root, m),
        "the base holds what the hub sent"
    );
    assert_eq!(
        git(m, &["diff", "--cached", "--stat"]),
        "",
        "the index is untouched"
    );

    // The agent works and commits; only its own work is past the base.
    write(&m.join("b.rs"), "two, by the agent");
    let tip = commit(m, "d.rs", "agent", "agent commit");
    assert_eq!(
        git(m, &["diff", "--name-only", BASE_REF, &worktree(&s.root, m)]),
        "b.rs\nd.rs"
    );

    // Its commits land on the hub as a branch of the same name.
    assert_eq!(
        bring_back(&s, "aura/reviewer-re36", None),
        Some(BranchFetch::Updated { tip: tip.clone() })
    );
    assert_eq!(git(&s.hub, &["rev-parse", "aura/reviewer-re36"]), tip);
    assert_eq!(bring_back(&s, "aura/reviewer-re36", Some(&tip)), None);

    // Pull: the base moves to the tree the hub took.
    let tree = git(m, &["rev-parse", "HEAD^{tree}"]);
    sh(&s.root, m, &workspace::mark_pulled(&tree).expect("script"));
    assert_eq!(
        git(m, &["rev-parse", &format!("{BASE_REF}^{{tree}}")]),
        tree
    );

    // A branch checked out on the hub is never moved under the user.
    let next = commit(m, "e.rs", "more", "second agent commit");
    git(&s.hub, &["stash", "-q", "-u"]);
    git(&s.hub, &["checkout", "-q", "aura/reviewer-re36"]);
    assert_eq!(
        bring_back(&s, "aura/reviewer-re36", Some(&tip)),
        Some(BranchFetch::CheckedOut)
    );
    git(&s.hub, &["checkout", "-q", "main"]);
    assert_eq!(
        bring_back(&s, "aura/reviewer-re36", Some(&tip)),
        Some(BranchFetch::Updated { tip: next })
    );

    // Nor one the user moved elsewhere.
    let theirs = git(
        &s.hub,
        &[
            "commit-tree",
            "-p",
            &first,
            "-m",
            "the user's own",
            &format!("{first}^{{tree}}"),
        ],
    );
    git(&s.hub, &["branch", "-f", "aura/reviewer-re36", &theirs]);
    commit(m, "f.rs", "again", "third agent commit");
    assert_eq!(
        bring_back(&s, "aura/reviewer-re36", Some(&tip)),
        Some(BranchFetch::Diverged)
    );

    // A fork moves to a branch of its own and keeps the work.
    let parent_tip = git(m, &["rev-parse", "HEAD"]);
    sh(
        &s.root,
        m,
        &workspace::fork_branch("aura/helper-hp1").expect("script"),
    );
    assert_eq!(git(m, &["branch", "--show-current"]), "aura/helper-hp1");
    assert_eq!(git(m, &["rev-parse", "HEAD"]), parent_tip);
    assert!(
        git(m, &["for-each-ref", "--format=%(refname)", "refs/heads"])
            .lines()
            .all(|r| r.starts_with("refs/heads/aura/")),
        "the machine has only agent branches"
    );
}

#[test]
fn a_hub_at_the_snapshot_sends_no_bundle_and_a_clean_tree_no_delta() {
    let s = setup();
    let snap = snapshot(&s);
    assert_eq!(place(&s, &snap, "aura/a-1"), snap);
    assert!(!s.root.join("aura-head.bundle").exists());
    assert!(!s.root.join("aura-delta.tgz").exists());
    assert_eq!(git(&s.machine, &["branch", "--show-current"]), "aura/a-1");
    assert_eq!(git(&s.machine, &["status", "--porcelain"]), "");
    assert_eq!(bring_back(&s, "aura/a-1", None), None);
}
