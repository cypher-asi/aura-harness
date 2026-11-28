use super::*;
use aura_exec_tools::git_tool::{git_ship_impl, PushPolicy, PushTarget, ShipRequest};
use std::time::Duration;

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

fn has_git() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

/// A project repo with one commit on `main`, pushed to a bare origin,
/// and a data dir for the worktrees.
fn project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin.git");
    let repo = root.path().join("project");
    std::fs::create_dir_all(&repo).unwrap();
    git(
        root.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    git(&repo, &["init", "-q", "-b", "main"]);
    for (key, value) in [("user.name", "t"), ("user.email", "t@t")] {
        git(&repo, &["config", key, value]);
    }
    std::fs::create_dir_all(repo.join("app")).unwrap();
    std::fs::write(repo.join("app/README"), "hello\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "first"]);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "origin", "main"]);
    let data = root.path().join("data");
    (root, repo, origin, data)
}

fn ship(folder: &Path) -> aura_exec_tools::git_tool::Shipped {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime
        .block_on(git_ship_impl(
            folder,
            ShipRequest {
                message: "my work",
                target: "main",
                push: PushTarget::default(),
                timeout: Duration::from_secs(20),
                push_policy: PushPolicy::single(Duration::from_secs(20)),
            },
        ))
        .unwrap()
}

#[test]
fn two_members_ship_without_touching_each_others_files() {
    if !has_git() {
        return;
    }
    let (_root, repo, origin, data) = project();
    let mut folders = Vec::new();
    for (hex, def) in [("aaaa1111", "rust-engineer"), ("bbbb2222", "reviewer")] {
        let branch = member_branch(Some(def), hex);
        let folder = provision_member_worktree(&repo, &member_worktree_path(&data, hex), &branch)
            .unwrap()
            .unwrap();
        folders.push((folder, branch));
    }
    let [(a, a_branch), (b, b_branch)] = folders.as_slice() else {
        unreachable!()
    };
    assert_eq!(a_branch, "aura/rust-engineer-aaaa1111");
    assert_ne!(a, b);
    std::fs::write(a.join("a.txt"), "member a\n").unwrap();
    std::fs::write(b.join("b.txt"), "member b\n").unwrap();

    let shipped = ship(a);
    assert_eq!(&shipped.branch, a_branch);
    let files = git(&origin, &["ls-tree", "-r", "--name-only", a_branch]);
    assert!(
        files.contains("a.txt") && !files.contains("b.txt"),
        "{files}"
    );
    // B's work is where B left it, uncommitted, and the project folder
    // itself is untouched.
    assert_eq!(git(b, &["status", "--porcelain"]), "?? b.txt");
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");

    let shipped = ship(b);
    assert_eq!(&shipped.branch, b_branch);
    let files = git(&origin, &["ls-tree", "-r", "--name-only", b_branch]);
    assert!(
        files.contains("b.txt") && !files.contains("a.txt"),
        "{files}"
    );
}

#[test]
fn a_project_below_the_repos_top_maps_into_the_worktree() {
    if !has_git() {
        return;
    }
    let (_root, repo, _origin, data) = project();
    let worktree = member_worktree_path(&data, "cccc3333");
    let folder = provision_member_worktree(&repo.join("app"), &worktree, "aura/x-cccc3333")
        .unwrap()
        .unwrap();
    assert_eq!(folder, worktree.join("app"));
    assert!(folder.join("README").exists());
    // Again: the worktree is there already, and reused.
    let again = provision_member_worktree(&repo.join("app"), &worktree, "aura/x-cccc3333").unwrap();
    assert_eq!(again, Some(folder));
}

#[test]
fn a_folder_outside_git_gets_no_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let got = provision_member_worktree(dir.path(), &dir.path().join("w"), "aura/x").unwrap();
    assert_eq!(got, None);
}

#[test]
fn archiving_removes_a_clean_worktree_keeps_a_dirty_one_and_always_the_branch() {
    if !has_git() {
        return;
    }
    let (_root, repo, _origin, data) = project();
    let clean = member_worktree_path(&data, "dddd4444");
    let dirty = member_worktree_path(&data, "eeee5555");
    provision_member_worktree(&repo, &clean, "aura/a-dddd4444").unwrap();
    provision_member_worktree(&repo, &dirty, "aura/b-eeee5555").unwrap();
    std::fs::write(dirty.join("wip.txt"), "unsaved\n").unwrap();

    assert_eq!(remove_member_worktree(&clean).unwrap(), Removal::Removed);
    assert!(!clean.exists());
    assert_eq!(remove_member_worktree(&dirty).unwrap(), Removal::Kept);
    assert!(dirty.join("wip.txt").exists());
    assert_eq!(remove_member_worktree(&clean).unwrap(), Removal::Missing);
    let branches = git(&repo, &["branch", "--list", "aura/*"]);
    assert!(
        branches.contains("aura/a-dddd4444"),
        "the branch stays: {branches}"
    );

    // Restoring adds it back on its branch.
    provision_member_worktree(&repo, &clean, "aura/a-dddd4444").unwrap();
    assert_eq!(
        git(&clean, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "aura/a-dddd4444"
    );
}
