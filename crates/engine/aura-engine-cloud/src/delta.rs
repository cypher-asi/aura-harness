//! The project files a machine gets.
//!
//! - A **snapshot build** gets the project as committed: `git archive`
//!   of a commit.
//! - A **new agent machine** gets the delta on top of its snapshot: the
//!   tracked files changed since the snapshot's commit, the untracked
//!   files `.gitignore` lets through, and the list of deleted files.
//!
//! ## Invariants
//!
//! - `.env` and `.env.*` files (except `.env.example`) are never packed,
//!   whatever `.gitignore` says, and neither is anything under `.git/`,
//!   `.aura/`, `aura_data/` or `target/` ([`is_denied`]).
//! - A delta larger than [`MAX_DELTA_BYTES`] is refused rather than
//!   uploaded; the caller rebuilds the snapshot instead.
//! - Packing is blocking IO: call it from `spawn_blocking`.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::error::{CloudError, CloudResult};

/// Largest delta archive uploaded to a machine.
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

/// The committed project at `commit` as a gzipped tar (`git archive`),
/// for snapshot builds. Denied paths are filtered out on the machine
/// side by [`APPLY_ARCHIVE`]'s exclude list.
///
/// # Errors
///
/// When git fails.
pub fn archive_commit(project: &Path, commit: &str) -> CloudResult<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["archive", "--format=tar.gz", commit])
        .output()
        .map_err(|err| CloudError::io("run git archive", err))?;
    if !output.status.success() {
        return Err(CloudError::io(
            format!("git archive {commit}"),
            std::io::Error::other(String::from_utf8_lossy(&output.stderr).trim().to_string()),
        ));
    }
    Ok(output.stdout)
}

/// Shell run in a machine to apply a delta uploaded to
/// `/tmp/aura-delta.tgz`.
pub const APPLY_DELTA: &str = "set -e; cd \"$AURA_WORKSPACE\"; \
tar -xzf /tmp/aura-delta.tgz; \
if [ -s .aura-deleted ]; then tr '\\n' '\\0' < .aura-deleted | xargs -0 rm -f --; fi; \
rm -f .aura-deleted /tmp/aura-delta.tgz";

/// Shell run in a builder to unpack a `git archive` uploaded to
/// `/tmp/aura-project.tgz` and commit it, so the machine can diff its
/// work against the snapshot later.
pub const APPLY_ARCHIVE: &str = "set -e; mkdir -p \"$AURA_WORKSPACE\"; cd \"$AURA_WORKSPACE\"; \
tar -xzf /tmp/aura-project.tgz --exclude='.env' --exclude='.env.*' --exclude='.git'; \
rm -f /tmp/aura-project.tgz; \
git init -q 2>/dev/null || true; git add -A; \
git -c user.name=aura -c user.email=aura@localhost commit -q -m snapshot --allow-empty";

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn env_files_and_build_dirs_are_denied() {
        for denied in [
            ".env",
            ".env.local",
            "sub/.env",
            "sub/.env.production",
            ".git/config",
            "target/debug/aura",
            "aura_data/db/LOG",
            "../outside",
            "/etc/passwd",
        ] {
            assert!(is_denied(Path::new(denied)), "{denied}");
        }
        for allowed in [
            ".env.example",
            "src/main.rs",
            "docs/env.md",
            "my.envrc",
            "a/target.rs",
        ] {
            assert!(!is_denied(Path::new(allowed)), "{allowed}");
        }
    }

    fn sh(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        sh(dir.path(), &["init", "-q"]);
        sh(dir.path(), &["config", "user.email", "t@t"]);
        sh(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.rs"), "fn a() {}").expect("write");
        std::fs::write(dir.path().join("gone.rs"), "x").expect("write");
        sh(dir.path(), &["add", "-A"]);
        sh(dir.path(), &["commit", "-q", "-m", "base"]);
        dir
    }

    #[test]
    fn the_delta_never_holds_env_files_even_without_gitignore() {
        let dir = repo();
        let base = head_commit(dir.path()).expect("head");
        std::fs::write(dir.path().join("a.rs"), "fn a() { 1; }").expect("write");
        std::fs::write(dir.path().join("new.rs"), "fn n() {}").expect("write");
        std::fs::write(dir.path().join(".env"), "BOXD_API_KEY=bxd_secret").expect("write");
        std::fs::write(dir.path().join(".env.local"), "X=1").expect("write");
        std::fs::remove_file(dir.path().join("gone.rs")).expect("rm");

        let delta = collect(dir.path(), &base).expect("delta");
        assert_eq!(
            delta.files,
            vec![PathBuf::from("a.rs"), PathBuf::from("new.rs")]
        );
        assert_eq!(delta.deleted, vec![PathBuf::from("gone.rs")]);

        let packed = pack(dir.path(), &delta).expect("pack");
        let mut text = Vec::new();
        flate2::read::GzDecoder::new(packed.as_slice())
            .read_to_end(&mut text)
            .expect("gunzip");
        let text = String::from_utf8_lossy(&text);
        assert!(!text.contains("bxd_secret"));
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(packed.as_slice()));
        let names: Vec<String> = archive
            .entries()
            .expect("entries")
            .map(|e| {
                e.expect("entry")
                    .path()
                    .expect("path")
                    .display()
                    .to_string()
            })
            .collect();
        assert_eq!(names, vec!["a.rs", "new.rs", ".aura-deleted"]);
    }

    #[test]
    fn a_clean_tree_has_an_empty_delta() {
        let dir = repo();
        let base = head_commit(dir.path()).expect("head");
        let delta = collect(dir.path(), &base).expect("delta");
        assert!(delta.is_empty());
        assert!(!delta.wants_rebuild());
        assert!(!archive_commit(dir.path(), &base)
            .expect("archive")
            .is_empty());
    }
}
