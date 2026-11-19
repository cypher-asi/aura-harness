//! What changed in an isolated copy, and carrying changes between the
//! user's workspace and a copy.
//!
//! - [`carry_changes`] brings the user's uncommitted work into a fresh
//!   worktree, which `git worktree add` checks out at `HEAD` only.
//! - [`baseline`] records the copy as it is before an agent works in it,
//!   as a git tree; [`diff_since`] is then exactly what the agent changed.
//! - [`apply_patch`] puts such a diff into the user's workspace.
//! - [`snapshot`] records a folder of the user's own git working tree, and
//!   [`Snapshot::restore`] puts its files back as they were then (a failed
//!   subtask's changes, before it is retried).
//!
//! A plain copy (the workspace is not a git repository) gets a git
//! directory *beside* it rather than inside, so the agent working in the
//! copy sees the same files the user has, and nothing else.
//!
//! ## Invariants
//!
//! - Only local subcommands run (`rev-parse`, `diff`, `apply`,
//!   `ls-files`, `init`, `add`, `write-tree`, `diff-index`, `diff-tree`,
//!   `read-tree`, `checkout-index`); nothing here commits, fetches or
//!   pushes, and terminal prompts are off.
//! - Nothing writes to the user's repository except [`apply_patch`] and
//!   [`Snapshot::restore`], and only to the working tree; the baseline
//!   and snapshot trees are unreferenced objects that `git gc` drops in
//!   time.
//! - A snapshot stages into a scratch copy of the index
//!   (`GIT_INDEX_FILE`), so the user's index, `HEAD`, branches and stash
//!   are never changed. A restore never touches files git ignored when
//!   the snapshot was taken, nor anything under `.git`.
//!
//! ## Failure modes
//!
//! Every function returns [`IsolationError::Git`] with git's stderr when
//! a subcommand fails, and [`IsolationError::Io`] when git can't start or
//! a file can't be copied.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::IsolationError;

/// Directories that are build output, dependencies or Aura's own data:
/// left out of plain copies and of the untracked files carried into a
/// worktree.
pub const BUILD_OUTPUT: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "__pycache__",
    ".venv",
    "aura_data",
];

/// A `git` command for the working tree `root`, using `git_dir` instead
/// of the one git would find there when given.
fn git(root: &Path, git_dir: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    if let Some(git_dir) = git_dir {
        command
            .arg(format!("--git-dir={}", git_dir.display()))
            .arg(format!("--work-tree={}", root.display()));
    }
    command
}

/// Run `command` and return its stdout, or its stderr as the error.
fn output(mut command: Command, what: &str) -> Result<Vec<u8>, IsolationError> {
    let out = command.stdin(Stdio::null()).output()?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(IsolationError::Git(if stderr.is_empty() {
        format!("git {what} exited with {}", out.status)
    } else {
        format!("git {what}: {stderr}")
    }))
}

/// Run `command` with `input` on stdin.
fn with_input(mut command: Command, input: &[u8], what: &str) -> Result<(), IsolationError> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input)?;
    }
    let out = child.wait_with_output()?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(IsolationError::Git(format!("git {what}: {stderr}")))
}

/// The root of the git working tree `dir` is in, or `None` when it is in
/// none (or git is missing).
#[must_use]
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    let mut command = git(dir, None);
    command.args(["rev-parse", "--show-toplevel"]);
    let out = output(command, "rev-parse").ok()?;
    let root = String::from_utf8(out).ok()?;
    let root = PathBuf::from(root.trim());
    root.canonicalize().ok().or(Some(root))
}

/// What [`carry_changes`] brought over.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Carried {
    /// Whether there were uncommitted changes to tracked files, which
    /// were applied.
    pub tracked: bool,
    /// Untracked files copied.
    pub untracked: usize,
}

fn skipped(relative: &Path, skip: &[&str]) -> bool {
    relative
        .components()
        .any(|part| skip.iter().any(|name| part.as_os_str() == *name))
}

/// Bring the uncommitted work in the repository at `source` (changes to
/// tracked files, and untracked files git does not ignore, except under
/// `skip`) into `worktree`, a checkout of the same `HEAD`.
///
/// # Errors
/// Fails when the changes don't apply (the caller can still run on
/// `HEAD` alone) or a file can't be copied.
pub fn carry_changes(
    source: &Path,
    worktree: &Path,
    skip: &[&str],
) -> Result<Carried, IsolationError> {
    let mut carried = Carried::default();
    let mut diff = git(source, None);
    diff.args(["diff", "HEAD", "--binary", "--no-color", "--no-ext-diff"]);
    let patch = output(diff, "diff HEAD")?;
    if !patch.is_empty() {
        let mut apply = git(worktree, None);
        apply.args(["apply", "--binary", "--whitespace=nowarn", "-"]);
        with_input(apply, &patch, "apply")?;
        carried.tracked = true;
    }
    let mut untracked = git(source, None);
    untracked.args(["ls-files", "--others", "--exclude-standard", "-z"]);
    let listed = output(untracked, "ls-files")?;
    for name in listed.split(|b| *b == 0).filter(|name| !name.is_empty()) {
        let relative = PathBuf::from(String::from_utf8_lossy(name).into_owned());
        if skipped(&relative, skip) {
            continue;
        }
        let from = source.join(&relative);
        if !from.is_file() {
            continue;
        }
        let to = worktree.join(&relative);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&from, &to)?;
        carried.untracked += 1;
    }
    Ok(carried)
}

/// Record the working tree `root` as it is now and return its git tree.
/// With `git_dir`, `root` is a plain copy: the repository there is
/// created on first use, ignoring [`BUILD_OUTPUT`].
///
/// # Errors
/// Fails when git can't stage or write the tree.
pub fn baseline(root: &Path, git_dir: Option<&Path>) -> Result<String, IsolationError> {
    if let Some(git_dir) = git_dir.filter(|dir| !dir.exists()) {
        let mut init = git(root, Some(git_dir));
        init.args(["init", "--quiet"]);
        output(init, "init")?;
        let info = git_dir.join("info");
        std::fs::create_dir_all(&info)?;
        let excluded: String = BUILD_OUTPUT
            .iter()
            .map(|name| format!("/{name}/\n"))
            .collect();
        std::fs::write(info.join("exclude"), excluded)?;
    }
    stage(root, git_dir)?;
    let mut write_tree = git(root, git_dir);
    write_tree.arg("write-tree");
    let tree = output(write_tree, "write-tree")?;
    Ok(String::from_utf8_lossy(&tree).trim().to_string())
}

fn stage(root: &Path, git_dir: Option<&Path>) -> Result<(), IsolationError> {
    let mut add = git(root, git_dir);
    add.args(["add", "--all", "--", "."]);
    output(add, "add").map(|_| ())
}

/// Everything that changed in `root` since the [`baseline`] `tree`, as a
/// patch [`apply_patch`] can apply; empty when nothing did. Folders named
/// by the relative paths in `leave_out`, at any depth, are not in it.
///
/// # Errors
/// Fails when git can't stage or diff the tree.
pub fn diff_since(
    root: &Path,
    git_dir: Option<&Path>,
    tree: &str,
    leave_out: &[&str],
) -> Result<String, IsolationError> {
    stage(root, git_dir)?;
    let mut diff = git(root, git_dir);
    diff.args([
        "diff-index",
        "--cached",
        "--patch",
        "--binary",
        "--no-color",
        "--no-ext-diff",
        tree,
        "--",
        ".",
    ]);
    diff.args(
        leave_out
            .iter()
            .map(|path| format!(":(exclude,glob)**/{path}/**")),
    );
    let patch = output(diff, "diff-index")?;
    Ok(String::from_utf8_lossy(&patch).into_owned())
}

/// Files `patch` (as [`diff_since`] writes it) changes.
#[must_use]
pub fn files_in(patch: &str) -> usize {
    patch
        .lines()
        .filter(|line| line.starts_with("diff --git "))
        .count()
}

/// Apply `patch` to the files under `dest`, which need not be a git
/// repository. Nothing changes when any part of it does not apply.
///
/// # Errors
/// Fails with git's explanation when the patch does not apply.
pub fn apply_patch(dest: &Path, patch: &str) -> Result<(), IsolationError> {
    if patch.trim().is_empty() {
        return Ok(());
    }
    let mut apply = git(dest, None);
    apply.args(["apply", "--binary", "--whitespace=nowarn", "-"]);
    with_input(apply, patch.as_bytes(), "apply")
}

/// Mode git gives a nested repository (a submodule or an embedded clone);
/// a restore leaves those alone.
const GITLINK: &str = "160000";

/// A folder of a git working tree as it was at one moment: its tracked
/// and untracked files git does not ignore. See [`snapshot`].
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Root of the working tree
    root: PathBuf,
    /// The folder recorded, relative to `root` (`.` for all of it)
    scope: PathBuf,
    /// Its files, as a git tree
    tree: String,
    /// Untracked files and folders (ending in `/`) git ignored then,
    /// relative to `root`
    ignored: Vec<String>,
}

/// What [`Snapshot::restore`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Restored {
    /// Files written back as they were
    pub written: Vec<PathBuf>,
    /// Files the snapshot did not have, removed
    pub removed: Vec<PathBuf>,
    /// Files not written back because a folder that still holds files
    /// (ignored ones, or a nested repository) stands where they go
    pub blocked: Vec<PathBuf>,
}

/// An index file of its own, in a temporary folder, so staging never
/// changes the user's index.
struct ScratchIndex {
    dir: tempfile::TempDir,
}

impl ScratchIndex {
    /// An index with nothing in it yet.
    fn empty() -> Result<Self, IsolationError> {
        let dir = tempfile::Builder::new().prefix("aura-index-").tempdir()?;
        Ok(Self { dir })
    }

    /// A copy of the index of the repository at `root`. Staging into a
    /// copy keeps files the repository tracks even where an ignore rule
    /// matches them, and reuses the index's file stats.
    fn copy_of(root: &Path) -> Result<Self, IsolationError> {
        let scratch = Self::empty()?;
        let mut git_path = git(root, None);
        git_path.args(["rev-parse", "--git-path", "index"]);
        let index = output(git_path, "rev-parse --git-path")?;
        let index = root.join(String::from_utf8_lossy(&index).trim());
        if index.is_file() {
            std::fs::copy(&index, scratch.path())?;
        }
        Ok(scratch)
    }

    fn path(&self) -> PathBuf {
        self.dir.path().join("index")
    }

    /// A `git` command for the working tree `root` using this index.
    fn git(&self, root: &Path) -> Command {
        let mut command = literal_git(root);
        command.env("GIT_INDEX_FILE", self.path());
        command
    }
}

/// A `git` command for the working tree `root` that takes pathspecs as
/// plain paths.
fn literal_git(root: &Path) -> Command {
    let mut command = git(root, None);
    command.env("GIT_LITERAL_PATHSPECS", "1");
    command
}

/// The tracked and untracked, non-ignored files under `scope` in the
/// working tree `root` as they are now, as a git tree.
fn worktree_tree(root: &Path, scope: &Path) -> Result<String, IsolationError> {
    let index = ScratchIndex::copy_of(root)?;
    let mut add = index.git(root);
    add.args(["add", "--all", "--"]).arg(scope);
    output(add, "add")?;
    let mut write_tree = index.git(root);
    write_tree.arg("write-tree");
    let tree = output(write_tree, "write-tree")?;
    Ok(String::from_utf8_lossy(&tree).trim().to_string())
}

/// The NUL-separated names in `listed`.
fn names(listed: &[u8]) -> impl Iterator<Item = String> + '_ {
    listed
        .split(|b| *b == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
}

/// Record the files under `workspace` in the git working tree it is in:
/// tracked files and untracked ones git does not ignore, as they are now,
/// uncommitted changes included. `None` when `workspace` is in no git
/// working tree.
///
/// Only objects are written to the repository; its index, `HEAD`,
/// branches and stash stay as they are.
///
/// # Errors
/// Fails when git can't stage, write the tree or list ignored files.
pub fn snapshot(workspace: &Path) -> Result<Option<Snapshot>, IsolationError> {
    let Some(root) = repo_root(workspace) else {
        return Ok(None);
    };
    let workspace = workspace.canonicalize()?;
    let Ok(relative) = workspace.strip_prefix(&root) else {
        return Ok(None);
    };
    let scope = if relative.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        relative.to_path_buf()
    };
    let tree = worktree_tree(&root, &scope)?;
    let mut ls = literal_git(&root);
    ls.args([
        "ls-files",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--directory",
        "-z",
        "--",
    ])
    .arg(&scope);
    let ignored = names(&output(ls, "ls-files")?).collect();
    Ok(Some(Snapshot {
        root,
        scope,
        tree,
        ignored,
    }))
}

impl Snapshot {
    /// Whether `path` (relative to the root) was an ignored file, or in
    /// an ignored folder, when the snapshot was taken.
    fn was_ignored(&self, path: &str) -> bool {
        self.ignored.iter().any(|entry| {
            if entry.ends_with('/') {
                path.starts_with(entry.as_str())
            } else {
                path == entry
            }
        })
    }

    /// Put the recorded folder back as it was when the snapshot was taken:
    /// files changed or deleted since are written back from the snapshot,
    /// and files created since are removed. Files git ignored at the
    /// snapshot, nested repositories and `.git` are left alone, and
    /// folders emptied by a removal stay. A file replaced by a folder
    /// that still holds such files is not written back (see
    /// [`Restored::blocked`]).
    ///
    /// # Errors
    /// Fails when git can't stage or compare the files, or a file can't be
    /// removed or written back; what was done before the failure stays.
    pub fn restore(&self) -> Result<Restored, IsolationError> {
        let now = worktree_tree(&self.root, &self.scope)?;
        let mut diff = literal_git(&self.root);
        diff.args([
            "diff-tree",
            "-r",
            "-z",
            "--raw",
            "--no-renames",
            self.tree.as_str(),
            now.as_str(),
            "--",
        ])
        .arg(&self.scope);
        let raw = output(diff, "diff-tree")?;
        let mut back = Vec::new();
        let mut created = Vec::new();
        let mut fields = names(&raw);
        while let (Some(meta), Some(path)) = (fields.next(), fields.next()) {
            let parts: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
            let [old_mode, new_mode, _, _, status] = parts[..] else {
                return Err(IsolationError::Git(format!(
                    "git diff-tree: unexpected line `{meta}`"
                )));
            };
            if old_mode == GITLINK || new_mode == GITLINK {
                continue;
            }
            if status == "A" {
                if !self.was_ignored(&path) {
                    created.push(path);
                }
            } else {
                back.push(path);
            }
        }

        let mut restored = Restored::default();
        for path in created {
            let full = self.root.join(&path);
            match std::fs::remove_file(&full) {
                Ok(()) => restored.removed.push(full),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        // `checkout-index --force` deletes a whole folder standing where a
        // file goes back, ignored files in it included; only an empty one
        // may go.
        back.retain(|path| {
            let full = self.root.join(path);
            let folder = full.symlink_metadata().is_ok_and(|meta| meta.is_dir());
            if folder && std::fs::remove_dir(&full).is_err() {
                restored.blocked.push(full);
                return false;
            }
            true
        });
        if !back.is_empty() {
            let index = ScratchIndex::empty()?;
            let mut read_tree = index.git(&self.root);
            read_tree.args(["read-tree", self.tree.as_str()]);
            output(read_tree, "read-tree")?;
            let mut checkout = index.git(&self.root);
            checkout.args(["checkout-index", "--force", "-z", "--stdin"]);
            let listed: Vec<u8> = back
                .iter()
                .flat_map(|path| path.bytes().chain(std::iter::once(0)))
                .collect();
            with_input(checkout, &listed, "checkout-index")?;
            restored
                .written
                .extend(back.iter().map(|path| self.root.join(path)));
        }
        Ok(restored)
    }
}

#[cfg(test)]
mod tests {
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
        std::fs::write(root.join("gone.rs"), "fn gone() {}\n").unwrap();
        std::fs::write(root.join("wip.rs"), "fn wip() {}\n").unwrap();
        run(&root, &["add", "gone.rs", "wip.rs"]);
        run(&root, &["commit", "--quiet", "-m", "more"]);
        std::fs::write(root.join("wip.rs"), "fn wip() { started() }\n").unwrap();
        std::fs::write(root.join("staged.rs"), "fn staged() {}\n").unwrap();
        run(&root, &["add", "staged.rs"]);
        std::fs::write(root.join("notes.md"), "mine\n").unwrap();
        std::fs::create_dir(root.join("target")).unwrap();
        std::fs::write(root.join("target/out"), "build\n").unwrap();
        let before = repository_state(&root);

        let snapshot = snapshot(&root).unwrap().expect("a git working tree");
        assert_eq!(
            repository_state(&root),
            before,
            "a snapshot changes nothing"
        );

        std::fs::write(root.join("lib.rs"), "fn a() { broken }\n").unwrap();
        std::fs::write(root.join("wip.rs"), "fn wip() { worse }\n").unwrap();
        std::fs::remove_file(root.join("gone.rs")).unwrap();
        std::fs::remove_file(root.join("notes.md")).unwrap();
        std::fs::create_dir_all(root.join("gen/deep")).unwrap();
        std::fs::write(root.join("gen/deep/made.rs"), "fn made() {}\n").unwrap();
        std::fs::write(root.join("target/out"), "rebuilt\n").unwrap();
        std::fs::write(root.join("target/new"), "build\n").unwrap();
        std::fs::write(root.join(".gitignore"), "").unwrap();

        let restored = snapshot.restore().unwrap();
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
        assert_eq!(repository_state(&root), before);

        assert_eq!(snapshot.restore().unwrap(), Restored::default());
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
}
