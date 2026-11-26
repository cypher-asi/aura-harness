//! The project a machine gets, and the commits it sends back.
//!
//! - A **snapshot build** gets the project's history: a `git bundle` of
//!   HEAD ([`bundle_head`]), cloned on the builder
//!   ([`crate::workspace::clone_snapshot`]).
//! - A **new agent machine** gets what the hub has on top of its
//!   snapshot: a bundle of the commits since the snapshot's commit
//!   ([`bundle_since`]), and the working tree's changes since HEAD
//!   ([`collect`], [`pack`]): the tracked files changed, the untracked
//!   files `.gitignore` lets through, and the list of deleted files.
//! - An agent's **commits come back** as a bundle of its branch, fetched
//!   into the project's branch of the same name ([`fetch_branch`]).
//!
//! ## Invariants
//!
//! - `.env` and `.env.*` files (except `.env.example`) are never packed
//!   from the working tree, whatever `.gitignore` says, and neither is
//!   anything under `.git/`, `.aura/`, `aura_data/` or `target/`
//!   ([`is_denied`]). Committed files travel with the history, as they
//!   would to the repository's remote.
//! - A delta or bundle larger than [`MAX_DELTA_BYTES`] is refused rather
//!   than uploaded.
//! - Fetching never moves a branch that is checked out or that the agent
//!   rewrote: git refuses, and the caller says so ([`BranchFetch`]).
//! - All of it is blocking IO: call it from `spawn_blocking`.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::error::{CloudError, CloudResult};

/// Largest delta archive or history bundle uploaded to a machine.
pub const MAX_DELTA_BYTES: u64 = 512 * 1024 * 1024;

/// Changed files past which the snapshot is rebuilt instead.
pub const REBUILD_AFTER_FILES: usize = 300;

/// Directories never packed.
const DENIED_DIRS: &[&str] = &[
    ".git",
    ".aura",
    "aura_data",
    "target",
    "target-agent",
    "node_modules",
];

/// Whether `path` (relative to the project) must never leave this
/// machine.
#[must_use]
pub fn is_denied(path: &Path) -> bool {
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            // `..`, roots and prefixes never belong to a project path.
            if matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            ) {
                return true;
            }
            continue;
        };
        let name = name.to_string_lossy();
        let is_last = components.peek().is_none();
        if !is_last && DENIED_DIRS.contains(&name.as_ref()) {
            return true;
        }
        if is_last {
            if name == ".env.example" {
                return false;
            }
            if name == ".env" || name.starts_with(".env.") {
                return true;
            }
        }
    }
    false
}

/// The files a machine needs on top of its snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Delta {
    /// Commit the snapshot holds.
    pub base_commit: String,
    /// Files to write, relative to the project.
    pub files: Vec<PathBuf>,
    /// Files to remove, relative to the project.
    pub deleted: Vec<PathBuf>,
}

impl Delta {
    /// Whether nothing changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.deleted.is_empty()
    }

    /// Whether the delta is big enough that rebuilding the snapshot is
    /// better than sending it.
    #[must_use]
    pub fn wants_rebuild(&self) -> bool {
        self.files.len() + self.deleted.len() > REBUILD_AFTER_FILES
    }
}

fn git(project: &Path, args: &[&str]) -> CloudResult<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(args)
        .output()
        .map_err(|err| CloudError::io(format!("run git {}", args.join(" ")), err))?;
    if !output.status.success() {
        return Err(CloudError::io(
            format!("git {} in {}", args.join(" "), project.display()),
            std::io::Error::other(String::from_utf8_lossy(&output.stderr).trim().to_string()),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn paths(listing: &str) -> impl Iterator<Item = PathBuf> + '_ {
    listing
        .split('\0')
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
}

/// The commit `HEAD` points at.
///
/// # Errors
///
/// When `project` is not a git repository with a commit.
pub fn head_commit(project: &Path) -> CloudResult<String> {
    Ok(git(project, &["rev-parse", "HEAD"])?.trim().to_string())
}

/// Whether `project` has `commit` (a snapshot's commit can be gone after
/// a rebase, a `git gc` or a fresh clone).
#[must_use]
pub fn has_commit(project: &Path, commit: &str) -> bool {
    git(
        project,
        &["cat-file", "-e", &format!("{commit}^{{commit}}")],
    )
    .is_ok()
}

/// What changed in `project` since `base_commit`, without denied files.
///
/// # Errors
///
/// When git fails (not a repository, unknown commit).
pub fn collect(project: &Path, base_commit: &str) -> CloudResult<Delta> {
    let changed = git(
        project,
        &[
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            "--diff-filter=d",
            base_commit,
        ],
    )?;
    let deleted = git(
        project,
        &[
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            "--diff-filter=D",
            base_commit,
        ],
    )?;
    let untracked = git(
        project,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    let mut files: Vec<PathBuf> = paths(&changed)
        .chain(paths(&untracked))
        .filter(|path| !is_denied(path) && project.join(path).is_file())
        .collect();
    files.sort();
    files.dedup();
    let mut deleted: Vec<PathBuf> = paths(&deleted).filter(|path| !is_denied(path)).collect();
    deleted.sort();
    Ok(Delta {
        base_commit: base_commit.to_string(),
        files,
        deleted,
    })
}

/// Pack `files` of `project` into a gzipped tar, with a `.aura-deleted`
/// list for `deleted`.
///
/// # Errors
///
/// When a file can't be read or the archive passes
/// [`MAX_DELTA_BYTES`].
pub fn pack(project: &Path, delta: &Delta) -> CloudResult<Vec<u8>> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut archive = tar::Builder::new(encoder);
    archive.follow_symlinks(false);
    let mut total = 0u64;
    for path in &delta.files {
        if is_denied(path) {
            continue;
        }
        let full = project.join(path);
        total += std::fs::metadata(&full)
            .map_err(|err| CloudError::io(format!("stat {}", full.display()), err))?
            .len();
        if total > MAX_DELTA_BYTES {
            return Err(CloudError::io(
                "pack project delta",
                std::io::Error::other(format!("larger than {MAX_DELTA_BYTES} bytes")),
            ));
        }
        archive
            .append_path_with_name(&full, path)
            .map_err(|err| CloudError::io(format!("pack {}", path.display()), err))?;
    }
    let deleted: String = delta
        .deleted
        .iter()
        .map(|path| format!("{}\n", path.display()))
        .collect();
    let mut header = tar::Header::new_gnu();
    header.set_size(deleted.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, ".aura-deleted", deleted.as_bytes())
        .map_err(|err| CloudError::io("pack deleted list", err))?;
    let encoder = archive
        .into_inner()
        .map_err(|err| CloudError::io("finish delta archive", err))?;
    encoder
        .finish()
        .map_err(|err| CloudError::io("compress delta archive", err))
}

/// The project's history up to HEAD, as a machine's snapshot gets it.
#[derive(Clone, PartialEq, Eq)]
pub struct HistoryBundle {
    /// HEAD's commit.
    pub commit: String,
    /// The `refs/remotes/origin/<branch>` HEAD's branch tracks, when the
    /// bundle carries it too: what this repository knows `origin` has.
    pub upstream: Option<String>,
    /// The bundle.
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for HistoryBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryBundle")
            .field("commit", &self.commit)
            .field("upstream", &self.upstream)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

fn repo_error(project: &Path, message: impl Into<String>) -> CloudError {
    CloudError::Repo {
        project: project.to_path_buf(),
        message: message.into(),
    }
}

/// `git bundle create` of `revs` in `project`, read back.
fn bundle(project: &Path, revs: &[&str]) -> CloudResult<Vec<u8>> {
    let dir = tempfile::tempdir().map_err(|err| CloudError::io("make a bundle folder", err))?;
    let path = dir.path().join("project.bundle");
    let path_arg = path.to_string_lossy().into_owned();
    let mut args = vec!["bundle", "create", "-q", path_arg.as_str()];
    args.extend_from_slice(revs);
    git(project, &args)?;
    let size = std::fs::metadata(&path)
        .map_err(|err| CloudError::io("stat the bundle", err))?
        .len();
    if size > MAX_DELTA_BYTES {
        return Err(repo_error(
            project,
            format!(
                "its history is {size} bytes as a git bundle, above the {MAX_DELTA_BYTES} a \
                 cloud machine is sent"
            ),
        ));
    }
    std::fs::read(&path).map_err(|err| CloudError::io("read the bundle", err))
}

/// The project's history up to HEAD, for a snapshot build.
///
/// # Errors
///
/// When `project` is not a repository with a commit, is a shallow clone
/// (its history can't be bundled), or the bundle is too large.
pub fn bundle_head(project: &Path) -> CloudResult<HistoryBundle> {
    let commit = head_commit(project)?;
    if git(project, &["rev-parse", "--is-shallow-repository"])?.trim() == "true" {
        return Err(repo_error(
            project,
            "it is a shallow clone, so its history can't go to a cloud machine; \
             run `git fetch --unshallow` first",
        ));
    }
    let upstream = git(
        project,
        &["rev-parse", "--symbolic-full-name", "HEAD@{upstream}"],
    )
    .ok()
    .map(|name| name.trim().to_string())
    .filter(|name| {
        name.strip_prefix("refs/remotes/origin/")
            .is_some_and(crate::workspace::is_branch_name)
    });
    let mut revs = vec!["HEAD"];
    if let Some(name) = upstream.as_deref() {
        revs.push(name);
    }
    let bytes = bundle(project, &revs)?;
    Ok(HistoryBundle {
        commit,
        upstream,
        bytes,
    })
}

/// The commits HEAD has that `base` (a snapshot's commit) lacks, as a
/// bundle; `None` when there are none.
///
/// # Errors
///
/// When git fails or the bundle is too large.
pub fn bundle_since(project: &Path, base: &str) -> CloudResult<Option<Vec<u8>>> {
    let not_base = format!("^{base}");
    let count = git(project, &["rev-list", "--count", "HEAD", &not_base])?;
    if count.trim() == "0" {
        return Ok(None);
    }
    bundle(project, &["HEAD", &not_base]).map(Some)
}

/// `origin`'s URL, when the project has one.
#[must_use]
pub fn origin_url(project: &Path) -> Option<String> {
    git(project, &["remote", "get-url", "origin"])
        .ok()
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
}

/// How fetching an agent's branch into the project went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchFetch {
    /// The branch now points at `tip`.
    Updated {
        /// The branch's new tip.
        tip: String,
    },
    /// The branch is checked out here, and git doesn't move a checked-out
    /// branch.
    CheckedOut,
    /// The branch here and the agent's have gone different ways.
    Diverged,
}

/// Fetch `branch` from the bundle at `bundle` into the project's branch
/// of the same name, fast-forward only.
///
/// # Errors
///
/// When git fails for another reason (a bundle needing commits the
/// project lost, an unreadable bundle).
pub fn fetch_branch(project: &Path, bundle: &Path, branch: &str) -> CloudResult<BranchFetch> {
    if !crate::workspace::is_branch_name(branch) {
        return Err(repo_error(project, format!("refusing branch {branch:?}")));
    }
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    let output = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["fetch", "--no-write-fetch-head"])
        .arg(bundle)
        .arg(&refspec)
        .output()
        .map_err(|err| CloudError::io("run git fetch", err))?;
    if output.status.success() {
        let tip = git(project, &["rev-parse", &format!("refs/heads/{branch}")])?;
        return Ok(BranchFetch::Updated {
            tip: tip.trim().to_string(),
        });
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("refusing to fetch into branch") || stderr.contains("checked out at") {
        return Ok(BranchFetch::CheckedOut);
    }
    if stderr.contains("non-fast-forward") || stderr.contains("[rejected]") {
        return Ok(BranchFetch::Diverged);
    }
    Err(repo_error(
        project,
        format!(
            "could not fetch {branch} from the machine: {}",
            stderr.trim().chars().take(300).collect::<String>()
        ),
    ))
}

/// Shell run, after a pull, in a machine made from a snapshot of the
/// older layout (no [`crate::workspace::BASE_REF`]): commit its work
/// there, since its patch is taken against `HEAD`.
pub const MARK_PULLED_LEGACY: &str = "set -e; cd \"$AURA_WORKSPACE\"; git add -A; \
git -c user.name=aura -c user.email=aura@localhost commit -q --allow-empty -m 'aura: pulled to the hub'";

#[cfg(test)]
mod tests;
