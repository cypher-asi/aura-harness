//! What changed in an isolated copy, and carrying changes between the
//! user's workspace and a copy.
//!
//! - [`carry_changes`] brings the user's uncommitted work into a fresh
//!   worktree, which `git worktree add` checks out at `HEAD` only.
//! - [`baseline`] records the copy as it is before an agent works in it,
//!   as a git tree; [`diff_since`] is then exactly what the agent changed.
//! - [`apply_patch`] puts such a diff into the user's workspace.
//!
//! A plain copy (the workspace is not a git repository) gets a git
//! directory *beside* it rather than inside, so the agent working in the
//! copy sees the same files the user has, and nothing else.
//!
//! ## Invariants
//!
//! - Only local subcommands run (`rev-parse`, `diff`, `apply`,
//!   `ls-files`, `init`, `add`, `write-tree`, `diff-index`); nothing here
//!   commits, fetches or pushes, and terminal prompts are off.
//! - Nothing writes to the user's repository except [`apply_patch`],
//!   and only to the working tree it is given; the baseline trees are
//!   unreferenced objects that `git gc` drops in time.
//!
//! ## Failure modes
//!
//! Every function returns [`IsolationError::Git`] with git's stderr when
//! a subcommand fails, and [`IsolationError::Io`] when git can't start or
//! a file can't be copied.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::IsolationError;

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
/// patch [`apply_patch`] can apply; empty when nothing did.
///
/// # Errors
/// Fails when git can't stage or diff the tree.
pub fn diff_since(
    root: &Path,
    git_dir: Option<&Path>,
    tree: &str,
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
    ]);
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
        assert_eq!(diff_since(copy.path(), Some(&git_dir), &tree).unwrap(), "");

        std::fs::write(copy.path().join("lib.rs"), "fn a() { b() }\n").unwrap();
        std::fs::write(copy.path().join("new.rs"), "fn b() {}\n").unwrap();
        std::fs::write(copy.path().join("target/more"), "build").unwrap();
        let patch = diff_since(copy.path(), Some(&git_dir), &tree).unwrap();
        assert_eq!(files_in(&patch), 2, "{patch}");
        assert!(!patch.contains("target/"), "build output stays out");

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
        let patch = diff_since(&worktree, None, &tree).unwrap();
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
    }
}
