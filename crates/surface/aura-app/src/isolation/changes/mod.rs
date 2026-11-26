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

use std::path::{Path, PathBuf};
use std::process::Command;

use super::IsolationError;

mod paths;
mod run;
mod snapshot;

pub use paths::{files_in, paths_in};
use paths::{names, skipped};
use run::{output, with_input};
pub use snapshot::{Restored, Snapshot};

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
    diff_index(root, git_dir, tree, leave_out, true)
}

/// [`diff_since`] to read rather than to apply: a binary file that
/// changed is named, its bytes left out.
///
/// # Errors
/// Fails when git can't stage or diff the tree.
pub fn readable_diff_since(
    root: &Path,
    git_dir: Option<&Path>,
    tree: &str,
    leave_out: &[&str],
) -> Result<String, IsolationError> {
    diff_index(root, git_dir, tree, leave_out, false)
}

fn diff_index(
    root: &Path,
    git_dir: Option<&Path>,
    tree: &str,
    leave_out: &[&str],
    binary: bool,
) -> Result<String, IsolationError> {
    stage(root, git_dir)?;
    let mut diff = git(root, git_dir);
    diff.args(["diff-index", "--cached", "--patch"]);
    if binary {
        diff.arg("--binary");
    }
    diff.args(["--no-color", "--no-ext-diff", tree, "--", "."]);
    diff.args(
        leave_out
            .iter()
            .map(|path| format!(":(exclude,glob)**/{path}/**")),
    );
    let patch = output(diff, "diff-index")?;
    Ok(String::from_utf8_lossy(&patch).into_owned())
}

/// Bytes in the untracked files under `workspace` that git does not
/// ignore: what a [`snapshot`] would have to read and store beyond the
/// tracked files. `None` when `workspace` is in no git working tree.
#[must_use]
pub fn untracked_bytes(workspace: &Path) -> Option<u64> {
    repo_root(workspace)?;
    let mut ls = literal_git(workspace);
    ls.args([
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
        "--",
        ".",
    ]);
    let listed = output(ls, "ls-files").ok()?;
    let workspace = workspace.canonicalize().ok()?;
    Some(
        names(&listed)
            .filter_map(|name| std::fs::symlink_metadata(workspace.join(name)).ok())
            .filter(std::fs::Metadata::is_file)
            .map(|meta| meta.len())
            .sum(),
    )
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
    /// Everything that changed in the recorded folder since the snapshot
    /// was taken, as a unified diff to read: files changed, created and
    /// deleted, by whatever means. Empty when nothing did. A binary file
    /// is named, its bytes left out. Paths are relative to the root of
    /// the working tree.
    ///
    /// # Errors
    /// Fails when git can't stage or compare the files.
    pub fn diff(&self) -> Result<String, IsolationError> {
        let now = worktree_tree(&self.root, &self.scope)?;
        if now == self.tree {
            return Ok(String::new());
        }
        let mut diff = literal_git(&self.root);
        diff.args([
            "diff-tree",
            "-r",
            "--patch",
            "--no-color",
            "--no-ext-diff",
            "--no-renames",
            self.tree.as_str(),
            now.as_str(),
            "--",
        ])
        .arg(&self.scope);
        let patch = output(diff, "diff-tree")?;
        Ok(String::from_utf8_lossy(&patch).into_owned())
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
        let raw = self.raw_diff(&now)?;
        let (mut back, created) = self.sort_changes(&raw)?;
        let mut restored = Restored::default();
        self.remove_created(created, &mut restored)?;
        self.keep_unblocked(&mut back, &mut restored);
        if !back.is_empty() {
            self.check_out(&back)?;
            restored
                .written
                .extend(back.iter().map(|path| self.root.join(path)));
        }
        Ok(restored)
    }

    /// The raw `diff-tree` from the snapshot's tree to `now`, NUL-separated.
    fn raw_diff(&self, now: &str) -> Result<Vec<u8>, IsolationError> {
        let mut diff = literal_git(&self.root);
        diff.args([
            "diff-tree",
            "-r",
            "-z",
            "--raw",
            "--no-renames",
            self.tree.as_str(),
            now,
            "--",
        ])
        .arg(&self.scope);
        output(diff, "diff-tree")
    }

    /// Write the files `back` (relative to the root) back from the
    /// snapshot's tree.
    fn check_out(&self, back: &[String]) -> Result<(), IsolationError> {
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
        with_input(checkout, &listed, "checkout-index")
    }
}

#[cfg(test)]
mod tests;
