use super::*;
use tempfile::TempDir;

fn git_available() -> bool {
    which::which("git").is_ok()
}

fn run(cwd: &Path, args: &[&str]) {
    let status = git(cwd, None).args(args).status().unwrap();
    assert!(status.success(), "git {args:?} failed in {cwd:?}");
}

fn repo() -> TempDir {
    let repo = TempDir::new().unwrap();
    run(repo.path(), &["init", "--quiet", "-b", "main"]);
    run(repo.path(), &["config", "user.email", "test@example.com"]);
    run(repo.path(), &["config", "user.name", "Test"]);
    std::fs::write(repo.path().join(".gitignore"), "target/\n").unwrap();
    std::fs::write(repo.path().join("lib.rs"), "fn a() {}\n").unwrap();
    run(repo.path(), &["add", "."]);
    run(repo.path(), &["commit", "--quiet", "-m", "seed"]);
    repo
}

#[test]
fn a_plain_copy_diffs_against_its_baseline_and_the_patch_applies() {
    if !git_available() {
        return;
    }
    let copy = TempDir::new().unwrap();
    let git_dir = TempDir::new().unwrap();
    let git_dir = git_dir.path().join("copy.git");
    std::fs::write(copy.path().join("lib.rs"), "fn a() {}\n").unwrap();
    std::fs::create_dir(copy.path().join("target")).unwrap();
    std::fs::write(copy.path().join("target/out"), "build").unwrap();

    let tree = baseline(copy.path(), Some(&git_dir)).unwrap();
    assert!(
        !copy.path().join(".git").exists(),
        "the git dir sits beside"
    );
    assert_eq!(
        diff_since(copy.path(), Some(&git_dir), &tree, &[]).unwrap(),
        ""
    );

    std::fs::write(copy.path().join("lib.rs"), "fn a() { b() }\n").unwrap();
    std::fs::write(copy.path().join("new.rs"), "fn b() {}\n").unwrap();
    std::fs::write(copy.path().join("target/more"), "build").unwrap();
    for records in [".aura/tasks", "sub/.aura/tasks"] {
        std::fs::create_dir_all(copy.path().join(records)).unwrap();
        std::fs::write(copy.path().join(records).join("1.json"), "{}").unwrap();
    }
    let patch = diff_since(copy.path(), Some(&git_dir), &tree, &[".aura/tasks"]).unwrap();
    assert_eq!(files_in(&patch), 2, "{patch}");
    assert!(!patch.contains("target/"), "build output stays out");
    assert!(!patch.contains(".aura/tasks"), "so do the left-out folders");

    let user = TempDir::new().unwrap();
    std::fs::write(user.path().join("lib.rs"), "fn a() {}\n").unwrap();
    apply_patch(user.path(), &patch).unwrap();
    assert_eq!(
        std::fs::read_to_string(user.path().join("lib.rs")).unwrap(),
        "fn a() { b() }\n"
    );
    assert!(user.path().join("new.rs").exists());
    assert!(apply_patch(user.path(), &patch).is_err(), "it applies once");
    apply_patch(user.path(), "").unwrap();
}

#[test]
fn uncommitted_work_is_carried_into_a_worktree() {
    if !git_available() {
        return;
    }
    let repo = repo();
    let root = repo.path().canonicalize().unwrap();
    std::fs::write(root.join("lib.rs"), "fn a() { /* wip */ }\n").unwrap();
    std::fs::write(root.join("notes.md"), "todo\n").unwrap();
    std::fs::create_dir_all(root.join("aura_data")).unwrap();
    std::fs::write(root.join("aura_data/db"), "x").unwrap();
    assert_eq!(repo_root(&root).as_deref(), Some(root.as_path()));

    let parent = TempDir::new().unwrap();
    let worktree = parent.path().join("wt");
    run(
        &root,
        &[
            "worktree",
            "add",
            "--detach",
            worktree.to_str().unwrap(),
            "HEAD",
        ],
    );
    let carried = carry_changes(&root, &worktree, BUILD_OUTPUT).unwrap();
    assert_eq!(
        carried,
        Carried {
            tracked: true,
            untracked: 1
        }
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("lib.rs")).unwrap(),
        "fn a() { /* wip */ }\n"
    );
    assert!(worktree.join("notes.md").exists());
    assert!(!worktree.join("aura_data").exists());

    let tree = baseline(&worktree, None).unwrap();
    std::fs::write(worktree.join("lib.rs"), "fn a() { done() }\n").unwrap();
    let patch = diff_since(&worktree, None, &tree, &[]).unwrap();
    assert_eq!(files_in(&patch), 1, "only the agent's change: {patch}");
    apply_patch(&root, &patch).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("lib.rs")).unwrap(),
        "fn a() { done() }\n"
    );
    run(
        &root,
        &["worktree", "remove", "--force", worktree.to_str().unwrap()],
    );
}

#[test]
fn a_folder_outside_git_has_no_repo_root() {
    if !git_available() {
        return;
    }
    let dir = TempDir::new().unwrap();
    assert_eq!(repo_root(dir.path()), None);
    assert!(snapshot(dir.path()).unwrap().is_none());
}

fn stdout(cwd: &Path, args: &[&str]) -> String {
    let out = git(cwd, None).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?} failed in {cwd:?}");
    String::from_utf8(out.stdout).unwrap()
}

fn read(path: PathBuf) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// `HEAD`, refs, stash list and the index file's bytes of `root`.
fn repository_state(root: &Path) -> (String, String, String, Vec<u8>) {
    (
        stdout(root, &["rev-parse", "HEAD"]),
        stdout(root, &["for-each-ref"]),
        stdout(root, &["stash", "list"]),
        std::fs::read(root.join(".git/index")).unwrap(),
    )
}

#[test]
fn a_restore_undoes_every_change_since_the_snapshot_and_nothing_before_it() {
    if !git_available() {
        return;
    }
    let repo = repo();
    let root = repo.path().canonicalize().unwrap();
    commit_and_start_work(&root);
    let before = repository_state(&root);

    let snapshot = snapshot(&root).unwrap().expect("a git working tree");
    assert_eq!(
        repository_state(&root),
        before,
        "a snapshot changes nothing"
    );

    change_everything(&root);

    let restored = snapshot.restore().unwrap();
    assert_put_back(&root, &restored);
    assert_eq!(repository_state(&root), before);

    assert_eq!(snapshot.restore().unwrap(), Restored::default());
}

/// Commit `gone.rs` and `wip.rs`, then leave uncommitted, staged,
/// untracked and ignored work in `root`.
fn commit_and_start_work(root: &Path) {
    std::fs::write(root.join("gone.rs"), "fn gone() {}\n").unwrap();
    std::fs::write(root.join("wip.rs"), "fn wip() {}\n").unwrap();
    run(root, &["add", "gone.rs", "wip.rs"]);
    run(root, &["commit", "--quiet", "-m", "more"]);
    std::fs::write(root.join("wip.rs"), "fn wip() { started() }\n").unwrap();
    std::fs::write(root.join("staged.rs"), "fn staged() {}\n").unwrap();
    run(root, &["add", "staged.rs"]);
    std::fs::write(root.join("notes.md"), "mine\n").unwrap();
    std::fs::create_dir(root.join("target")).unwrap();
    std::fs::write(root.join("target/out"), "build\n").unwrap();
}

/// Change, delete, create and rewrite files, ignored ones included.
fn change_everything(root: &Path) {
    std::fs::write(root.join("lib.rs"), "fn a() { broken }\n").unwrap();
    std::fs::write(root.join("wip.rs"), "fn wip() { worse }\n").unwrap();
    std::fs::remove_file(root.join("gone.rs")).unwrap();
    std::fs::remove_file(root.join("notes.md")).unwrap();
    std::fs::create_dir_all(root.join("gen/deep")).unwrap();
    std::fs::write(root.join("gen/deep/made.rs"), "fn made() {}\n").unwrap();
    std::fs::write(root.join("target/out"), "rebuilt\n").unwrap();
    std::fs::write(root.join("target/new"), "build\n").unwrap();
    std::fs::write(root.join(".gitignore"), "").unwrap();
}

/// The files of [`commit_and_start_work`] are back as they were.
fn assert_put_back(root: &Path, restored: &Restored) {
    assert_eq!(restored.removed, [root.join("gen/deep/made.rs")]);
    assert_eq!(restored.written.len(), 5, "{restored:?}");
    assert_eq!(read(root.join("lib.rs")), "fn a() {}\n");
    assert_eq!(read(root.join("gone.rs")), "fn gone() {}\n");
    assert_eq!(
        read(root.join("wip.rs")),
        "fn wip() { started() }\n",
        "uncommitted work from before stays"
    );
    assert_eq!(read(root.join("notes.md")), "mine\n");
    assert_eq!(read(root.join(".gitignore")), "target/\n");
    assert!(!root.join("gen/deep/made.rs").exists());
    assert_eq!(
        read(root.join("target/out")),
        "rebuilt\n",
        "ignored files are never touched"
    );
    assert!(root.join("target/new").exists());
    assert!(root.join(".git/HEAD").exists());
}

#[test]
fn a_snapshot_of_a_folder_restores_only_that_folder() {
    if !git_available() {
        return;
    }
    let repo = repo();
    let root = repo.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("app")).unwrap();
    std::fs::write(root.join("app/main.rs"), "fn main() {}\n").unwrap();
    run(&root, &["add", "app"]);
    run(&root, &["commit", "--quiet", "-m", "app"]);

    let snapshot = snapshot(&root.join("app")).unwrap().unwrap();
    std::fs::write(root.join("app/main.rs"), "fn main() { x }\n").unwrap();
    std::fs::write(root.join("app/new.rs"), "\n").unwrap();
    std::fs::write(root.join("lib.rs"), "fn a() { elsewhere }\n").unwrap();
    std::fs::write(root.join("outside.rs"), "\n").unwrap();

    let restored = snapshot.restore().unwrap();
    assert_eq!(restored.removed, [root.join("app/new.rs")]);
    assert_eq!(restored.written, [root.join("app/main.rs")]);
    assert_eq!(read(root.join("app/main.rs")), "fn main() {}\n");
    assert_eq!(read(root.join("lib.rs")), "fn a() { elsewhere }\n");
    assert!(root.join("outside.rs").exists());
}

#[test]
fn a_snapshot_diffs_everything_changed_since_by_whatever_means() {
    if !git_available() {
        return;
    }
    let repo = repo();
    let root = repo.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("app")).unwrap();
    std::fs::write(root.join("app/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("app/old.rs"), "fn old() {}\n").unwrap();
    std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();
    run(&root, &["add", "."]);
    run(&root, &["commit", "--quiet", "-m", "app"]);
    // Uncommitted work from before the snapshot is not part of the
    // change.
    std::fs::write(root.join("app/draft.rs"), "fn draft() {}\n").unwrap();
    let index_before = stdout(&root, &["status", "--porcelain"]);

    let snapshot = snapshot(&root.join("app")).unwrap().unwrap();
    assert_eq!(snapshot.diff().unwrap(), "", "nothing changed yet");
    assert_eq!(untracked_bytes(&root.join("app")), Some(14));
    std::fs::write(root.join("app/main.rs"), "fn main() { x }\n").unwrap();
    std::fs::write(root.join("app/new file.rs"), "fn new() {}\n").unwrap();
    std::fs::remove_file(root.join("app/old.rs")).unwrap();
    std::fs::write(root.join("app/run.log"), "noise\n").unwrap();
    std::fs::write(root.join("outside.rs"), "\n").unwrap();

    let diff = snapshot.diff().unwrap();
    assert!(diff.contains("-fn main() {}\n+fn main() { x }"), "{diff}");
    assert!(diff.contains("+fn new() {}"), "{diff}");
    assert!(diff.contains("-fn old() {}"), "{diff}");
    for absent in ["draft", "run.log", "outside.rs"] {
        assert!(!diff.contains(absent), "{absent}: {diff}");
    }
    assert_eq!(
        paths_in(&diff),
        ["app/main.rs", "app/new file.rs", "app/old.rs"]
    );
    assert_eq!(files_in(&diff), 3);
    // The user's index is as it was.
    run(&root, &["add", "outside.rs"]);
    run(&root, &["reset", "--quiet", "outside.rs"]);
    std::fs::remove_file(root.join("outside.rs")).unwrap();
    std::fs::write(root.join("app/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("app/old.rs"), "fn old() {}\n").unwrap();
    std::fs::remove_file(root.join("app/new file.rs")).unwrap();
    assert_eq!(stdout(&root, &["status", "--porcelain"]), index_before);
    assert_eq!(untracked_bytes(repo.path().parent().unwrap()), None);
}

#[test]
fn a_copys_diff_can_leave_binary_bytes_out() {
    if !git_available() {
        return;
    }
    let copy = TempDir::new().unwrap();
    let root = copy.path().join("copy");
    let git_dir = copy.path().join("copy.git");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    let tree = baseline(&root, Some(&git_dir)).unwrap();
    std::fs::write(root.join("a.txt"), "two\n").unwrap();
    std::fs::write(root.join("blob.bin"), [0u8, 159, 146, 150, 0, 1, 2]).unwrap();
    let readable = readable_diff_since(&root, Some(&git_dir), &tree, &[]).unwrap();
    assert!(readable.contains("-one\n+two"), "{readable}");
    assert!(readable.contains("Binary files"), "{readable}");
    assert!(!readable.contains("GIT binary patch"), "{readable}");
    assert_eq!(paths_in(&readable), ["a.txt", "blob.bin"]);
    let patch = diff_since(&root, Some(&git_dir), &tree, &[]).unwrap();
    assert!(patch.contains("GIT binary patch"), "{patch}");
}

#[test]
fn a_file_replaced_by_a_folder_comes_back_unless_ignored_files_are_in_it() {
    if !git_available() {
        return;
    }
    let repo = repo();
    let root = repo.path().canonicalize().unwrap();
    std::fs::write(root.join("plain"), "plain\n").unwrap();
    std::fs::write(root.join("cfg"), "cfg\n").unwrap();
    run(&root, &["add", "plain", "cfg"]);
    run(&root, &["commit", "--quiet", "-m", "files"]);

    let snapshot = snapshot(&root).unwrap().unwrap();
    std::fs::remove_file(root.join("plain")).unwrap();
    std::fs::create_dir(root.join("plain")).unwrap();
    std::fs::write(root.join("plain/made.rs"), "\n").unwrap();
    std::fs::remove_file(root.join("cfg")).unwrap();
    std::fs::create_dir_all(root.join("cfg/target")).unwrap();
    std::fs::write(root.join("cfg/target/out"), "build\n").unwrap();

    let restored = snapshot.restore().unwrap();
    assert_eq!(restored.removed, [root.join("plain/made.rs")]);
    assert_eq!(restored.written, [root.join("plain")]);
    assert_eq!(restored.blocked, [root.join("cfg")]);
    assert_eq!(read(root.join("plain")), "plain\n");
    assert_eq!(
        read(root.join("cfg/target/out")),
        "build\n",
        "an ignored file in the way is kept"
    );
}
