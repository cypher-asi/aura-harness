//! Pre-block file snapshots, so a failed attempt can be rolled back
//! before a stronger tier starts over.
//!
//! The snapshot holds every regular file under the workspace except
//! build output, dependencies and version-control data. Workspaces
//! over the size caps get no snapshot; their blocks escalate in place.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// Directory names never snapshotted or restored.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".worktrees",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    "dist",
    "build",
    ".next",
];

/// Size limits of a snapshot.
#[derive(Debug, Clone, Copy)]
pub struct Caps {
    /// Most files to hold.
    pub max_files: usize,
    /// Most bytes to hold.
    pub max_bytes: u64,
    /// Larger files are left out (and never restored).
    pub max_file_bytes: u64,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            max_files: 20_000,
            max_bytes: 256 * 1024 * 1024,
            max_file_bytes: 4 * 1024 * 1024,
        }
    }
}

/// Contents of a workspace at one moment.
#[derive(Debug)]
pub struct Snapshot {
    root: PathBuf,
    files: HashMap<PathBuf, Vec<u8>>,
    oversized: HashSet<PathBuf>,
}

/// What a restore changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Restored {
    /// Files written back.
    pub rewritten: usize,
    /// Files removed because the snapshot didn't have them.
    pub removed: usize,
}

fn files(root: &Path) -> impl Iterator<Item = walkdir::DirEntry> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            !(e.file_type().is_dir()
                && e.depth() > 0
                && e.file_name()
                    .to_str()
                    .is_some_and(|n| SKIP_DIRS.contains(&n)))
        })
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
}

impl Snapshot {
    /// Snapshot `root`, or `None` when it is over `caps` or unreadable.
    #[must_use]
    pub fn take(root: &Path, caps: Caps) -> Option<Self> {
        let mut snapshot = Self {
            root: root.to_path_buf(),
            files: HashMap::new(),
            oversized: HashSet::new(),
        };
        let mut bytes = 0u64;
        for entry in files(root) {
            let len = entry.metadata().ok()?.len();
            let rel = entry.path().strip_prefix(root).ok()?.to_path_buf();
            if len > caps.max_file_bytes {
                snapshot.oversized.insert(rel);
                continue;
            }
            bytes += len;
            if snapshot.files.len() >= caps.max_files || bytes > caps.max_bytes {
                return None;
            }
            snapshot
                .files
                .insert(rel, std::fs::read(entry.path()).ok()?);
        }
        Some(snapshot)
    }

    /// Files held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether the snapshot holds no files.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Put the workspace back: rewrite changed or deleted files and
    /// remove files created since.
    ///
    /// # Errors
    /// Fails on the first file that can't be written or removed.
    pub fn restore(&self) -> std::io::Result<Restored> {
        let mut restored = Restored::default();
        let mut seen = HashSet::new();
        for entry in files(&self.root) {
            let Ok(rel) = entry.path().strip_prefix(&self.root) else {
                continue;
            };
            let rel = rel.to_path_buf();
            if self.oversized.contains(&rel) {
                continue;
            }
            match self.files.get(&rel) {
                Some(content) => {
                    if std::fs::read(entry.path()).ok().as_deref() != Some(content.as_slice()) {
                        std::fs::write(entry.path(), content)?;
                        restored.rewritten += 1;
                    }
                }
                None => {
                    std::fs::remove_file(entry.path())?;
                    restored.removed += 1;
                }
            }
            seen.insert(rel);
        }
        for (rel, content) in &self.files {
            if seen.contains(rel) {
                continue;
            }
            let path = self.root.join(rel);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, content)?;
            restored.rewritten += 1;
        }
        Ok(restored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_undoes_edits_creations_and_deletions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("src/a.rs"), "a").unwrap();
        std::fs::write(root.join("b.txt"), "b").unwrap();
        std::fs::write(root.join("target/out"), "x").unwrap();

        let snap = Snapshot::take(root, Caps::default()).unwrap();
        assert_eq!(snap.len(), 2);

        std::fs::write(root.join("src/a.rs"), "changed").unwrap();
        std::fs::remove_file(root.join("b.txt")).unwrap();
        std::fs::write(root.join("src/new.rs"), "new").unwrap();
        std::fs::write(root.join("target/out"), "y").unwrap();

        let r = snap.restore().unwrap();
        assert_eq!(
            r,
            Restored {
                rewritten: 2,
                removed: 1
            }
        );
        assert_eq!(std::fs::read_to_string(root.join("src/a.rs")).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "b");
        assert!(!root.join("src/new.rs").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("target/out")).unwrap(),
            "y"
        );
    }

    #[test]
    fn over_caps_gives_no_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("{i}")), "x").unwrap();
        }
        let caps = Caps {
            max_files: 3,
            ..Caps::default()
        };
        assert!(Snapshot::take(dir.path(), caps).is_none());
    }
}
