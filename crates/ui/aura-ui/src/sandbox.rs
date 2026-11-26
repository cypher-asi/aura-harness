//! File tree of the active agent's sandbox, shown in the Sandbox panel.
//!
//! Directories are read only when expanded, so large trees cost nothing
//! until opened. Symlinks are listed but never followed, which keeps the
//! traversal inside the sandbox root.
//!
//! A cloud agent's sandbox is on its machine: the tree is then remote
//! ([`SandboxTree::set_remote_root`]) and shows the listings the app sends
//! for the directories it asks about ([`SandboxTree::wanted`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Entries listed per directory before the rest collapse into a "more" row.
const MAX_ENTRIES_PER_DIR: usize = 500;

/// What a row in the tree points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxEntryKind {
    /// Directory (expandable)
    Dir,
    /// Regular file
    File,
    /// Symbolic link (not followed)
    Symlink,
    /// Placeholder for entries hidden by the per-directory cap
    More(usize),
}

/// One visible row of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRow {
    /// Absolute path of the entry (the parent directory for `More` rows)
    pub path: PathBuf,
    /// File name
    pub name: String,
    /// Nesting level below the root
    pub depth: usize,
    /// Entry kind
    pub kind: SandboxEntryKind,
    /// Whether a directory row is expanded
    pub expanded: bool,
}

/// One entry of a directory listed on another machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    /// File name
    pub name: String,
    /// Whether it is a directory
    pub is_dir: bool,
}

/// What a remote tree knows of the other machine's directories.
#[derive(Debug, Default)]
struct Remote {
    /// Each listed directory's entries, by its path there
    listings: HashMap<PathBuf, Vec<RemoteEntry>>,
    /// Directories whose listing (or failure to list) is current
    fresh: HashSet<PathBuf>,
    /// Directories asked about and not answered yet
    asked: HashSet<PathBuf>,
    /// Why the root could not be listed, the last time it was asked
    failed: Option<String>,
}

/// Expandable file tree rooted at the agent's sandbox directory.
#[derive(Debug, Default)]
pub struct SandboxTree {
    root: Option<PathBuf>,
    expanded: HashSet<PathBuf>,
    rows: Vec<SandboxRow>,
    selected: usize,
    /// Set while the tree shows another machine's files
    remote: Option<Remote>,
}

impl SandboxTree {
    /// Point the tree at `root`. Switching to a different root collapses
    /// everything and moves the selection to the top.
    pub fn set_root(&mut self, root: PathBuf) {
        if self.root.as_ref() != Some(&root) || self.remote.is_some() {
            self.reset(root);
        }
        self.refresh();
    }

    /// Point the tree at `root` on another machine, whose directories are
    /// listed by asking ([`Self::wanted`]) rather than read here.
    /// Switching to a different root collapses everything and moves the
    /// selection to the top.
    pub fn set_remote_root(&mut self, root: PathBuf) {
        if self.root.as_ref() != Some(&root) || self.remote.is_none() {
            self.reset(root);
            self.remote = Some(Remote::default());
        }
        self.refresh();
    }

    fn reset(&mut self, root: PathBuf) {
        self.expanded.clear();
        self.selected = 0;
        self.rows.clear();
        self.root = Some(root);
        self.remote = None;
    }

    /// Sandbox root, once the backend has reported it.
    #[must_use]
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Whether the tree shows another machine's files.
    #[must_use]
    pub const fn is_remote(&self) -> bool {
        self.remote.is_some()
    }

    /// Whether a remote tree's root has no listing yet, and why the last
    /// ask failed when it did. `None` for a local tree or a listed root.
    #[must_use]
    pub fn unlisted(&self) -> Option<Option<&str>> {
        let remote = self.remote.as_ref()?;
        let root = self.root.as_ref()?;
        (!remote.listings.contains_key(root)).then_some(remote.failed.as_deref())
    }

    /// The directories a remote tree needs listed: its root and every
    /// open directory, unless current or asked about already. Mark each
    /// one asked for with [`Self::asked`].
    #[must_use]
    pub fn wanted(&self) -> Vec<PathBuf> {
        let (Some(remote), Some(root)) = (&self.remote, &self.root) else {
            return Vec::new();
        };
        std::iter::once(root)
            .chain(&self.expanded)
            .filter(|dir| !remote.fresh.contains(*dir) && !remote.asked.contains(*dir))
            .cloned()
            .collect()
    }

    /// `dir` was asked about; it is not wanted again until answered.
    pub fn asked(&mut self, dir: PathBuf) {
        if let Some(remote) = &mut self.remote {
            remote.asked.insert(dir);
        }
    }

    /// Show `entries` as what `dir` holds on the other machine.
    pub fn set_listing(&mut self, dir: PathBuf, entries: Vec<RemoteEntry>) {
        let Some(remote) = &mut self.remote else {
            return;
        };
        if self.root.as_ref() == Some(&dir) {
            remote.failed = None;
        }
        remote.asked.remove(&dir);
        remote.fresh.insert(dir.clone());
        remote.listings.insert(dir, entries);
        self.rebuild();
    }

    /// `dir` could not be listed, for `reason`: what shows of it stays,
    /// and it is asked about again on the next [`Self::refresh`].
    pub fn listing_failed(&mut self, dir: PathBuf, reason: String) {
        let Some(remote) = &mut self.remote else {
            return;
        };
        if self.root.as_ref() == Some(&dir) {
            remote.failed = Some(reason);
        }
        remote.asked.remove(&dir);
        remote.fresh.insert(dir);
    }

    /// Visible rows, in display order.
    #[must_use]
    pub fn rows(&self) -> &[SandboxRow] {
        &self.rows
    }

    /// Index of the highlighted row.
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// Re-read the root and every expanded directory, keeping the
    /// selection on the same path when it still exists. A remote tree
    /// keeps showing what it has until every directory is listed again.
    pub fn refresh(&mut self) {
        if let Some(remote) = &mut self.remote {
            remote.fresh.clear();
            remote.asked.clear();
        }
        self.rebuild();
    }

    /// Lay the rows out again from the disk, or a remote tree's listings.
    fn rebuild(&mut self) {
        let selected_path = self.rows.get(self.selected).map(|r| r.path.clone());
        self.rows.clear();
        if let Some(root) = self.root.clone() {
            let source = match &self.remote {
                Some(remote) => Source::Listed(&remote.listings),
                None => {
                    self.expanded.retain(|p| p.is_dir());
                    Source::Disk
                }
            };
            push_dir(&source, &root, 0, &self.expanded, &mut self.rows);
        }
        self.selected = selected_path
            .and_then(|path| self.rows.iter().position(|r| r.path == path))
            .unwrap_or(self.selected)
            .min(self.rows.len().saturating_sub(1));
    }

    /// Move the highlight `delta` rows, clamped to the list.
    pub fn move_by(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    /// Highlight row `index`, clamped to the list.
    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows.len().saturating_sub(1));
    }

    /// Highlight the first row.
    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    /// Highlight the last row.
    pub fn select_last(&mut self) {
        self.selected = self.rows.len().saturating_sub(1);
    }

    /// Expand or collapse the highlighted directory.
    pub fn toggle_selected(&mut self) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };
        if row.kind != SandboxEntryKind::Dir {
            return;
        }
        let path = row.path.clone();
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
        self.rebuild();
    }

    /// Collapse the highlighted directory, or move to its parent when it is
    /// already collapsed or not a directory.
    pub fn collapse_or_parent(&mut self) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };
        if row.kind == SandboxEntryKind::Dir && row.expanded {
            self.toggle_selected();
            return;
        }
        let Some(parent_depth) = row.depth.checked_sub(1) else {
            return;
        };
        if let Some(parent) = self.rows[..self.selected]
            .iter()
            .rposition(|r| r.depth == parent_depth)
        {
            self.selected = parent;
        }
    }
}

/// Where a tree's directories are read from.
enum Source<'a> {
    /// This computer's disk
    Disk,
    /// Listings of another machine's directories, by path there
    Listed(&'a HashMap<PathBuf, Vec<RemoteEntry>>),
}

impl Source<'_> {
    /// The name, path and kind of each entry of `dir`; `None` when it
    /// can't be read or has no listing.
    fn entries(&self, dir: &Path) -> Option<Vec<(String, PathBuf, SandboxEntryKind)>> {
        match self {
            Self::Disk => {
                let read_dir = std::fs::read_dir(dir).ok()?;
                Some(
                    read_dir
                        .filter_map(Result::ok)
                        .map(|entry| {
                            let kind = match entry.file_type() {
                                Ok(ft) if ft.is_symlink() => SandboxEntryKind::Symlink,
                                Ok(ft) if ft.is_dir() => SandboxEntryKind::Dir,
                                _ => SandboxEntryKind::File,
                            };
                            (
                                entry.file_name().to_string_lossy().into_owned(),
                                entry.path(),
                                kind,
                            )
                        })
                        .collect(),
                )
            }
            Self::Listed(listings) => Some(
                listings
                    .get(dir)?
                    .iter()
                    .map(|entry| {
                        let kind = if entry.is_dir {
                            SandboxEntryKind::Dir
                        } else {
                            SandboxEntryKind::File
                        };
                        (entry.name.clone(), dir.join(&entry.name), kind)
                    })
                    .collect(),
            ),
        }
    }
}

fn push_dir(
    source: &Source<'_>,
    dir: &Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    rows: &mut Vec<SandboxRow>,
) {
    let Some(mut entries) = source.entries(dir) else {
        return;
    };
    entries.sort_by(|a, b| {
        (a.2 != SandboxEntryKind::Dir)
            .cmp(&(b.2 != SandboxEntryKind::Dir))
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });

    let total = entries.len();
    for (name, path, kind) in entries.into_iter().take(MAX_ENTRIES_PER_DIR) {
        let is_open = kind == SandboxEntryKind::Dir && expanded.contains(&path);
        rows.push(SandboxRow {
            path: path.clone(),
            name,
            depth,
            kind,
            expanded: is_open,
        });
        if is_open {
            push_dir(source, &path, depth + 1, expanded, rows);
        }
    }
    if total > MAX_ENTRIES_PER_DIR {
        let hidden = total - MAX_ENTRIES_PER_DIR;
        rows.push(SandboxRow {
            path: dir.to_path_buf(),
            name: format!("… {hidden} more"),
            depth,
            kind: SandboxEntryKind::More(hidden),
            expanded: false,
        });
    }
}

#[cfg(test)]
mod tests;
