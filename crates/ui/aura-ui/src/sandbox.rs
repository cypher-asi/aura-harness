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
mod tests {
    use super::*;
    use std::fs;

    fn names(tree: &SandboxTree) -> Vec<String> {
        tree.rows()
            .iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.name))
            .collect()
    }

    fn sample() -> (tempfile::TempDir, SandboxTree) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "").unwrap();
        fs::write(dir.path().join("src/nested/deep.rs"), "").unwrap();
        fs::create_dir(dir.path().join("Docs")).unwrap();
        fs::write(dir.path().join("README.md"), "").unwrap();
        fs::write(dir.path().join(".env"), "").unwrap();
        let mut tree = SandboxTree::default();
        tree.set_root(dir.path().to_path_buf());
        (dir, tree)
    }

    #[test]
    fn lists_directories_first_case_insensitive() {
        let (_dir, tree) = sample();
        assert_eq!(names(&tree), ["Docs", "src", ".env", "README.md"]);
        assert_eq!(tree.rows()[0].kind, SandboxEntryKind::Dir);
        assert_eq!(tree.rows()[3].kind, SandboxEntryKind::File);
    }

    #[test]
    fn expand_and_collapse() {
        let (_dir, mut tree) = sample();
        tree.move_by(1);
        tree.toggle_selected();
        assert_eq!(
            names(&tree),
            ["Docs", "src", "  nested", "  main.rs", ".env", "README.md"]
        );
        assert!(tree.rows()[1].expanded);

        tree.toggle_selected();
        assert_eq!(names(&tree), ["Docs", "src", ".env", "README.md"]);
    }

    #[test]
    fn toggle_on_file_does_nothing() {
        let (_dir, mut tree) = sample();
        tree.select_last();
        tree.toggle_selected();
        assert_eq!(names(&tree).len(), 4);
    }

    #[test]
    fn left_collapses_then_jumps_to_parent() {
        let (_dir, mut tree) = sample();
        tree.move_by(1);
        tree.toggle_selected();
        tree.move_by(1);
        tree.toggle_selected();
        tree.move_by(1);
        assert_eq!(tree.rows()[tree.selected()].name, "deep.rs");

        tree.collapse_or_parent();
        assert_eq!(tree.rows()[tree.selected()].name, "nested");
        tree.collapse_or_parent();
        assert!(!tree.rows()[tree.selected()].expanded);
        tree.collapse_or_parent();
        assert_eq!(tree.rows()[tree.selected()].name, "src");
    }

    #[test]
    fn refresh_keeps_selection_and_picks_up_new_files() {
        let (dir, mut tree) = sample();
        tree.select_last();
        fs::write(dir.path().join("a.txt"), "").unwrap();
        tree.refresh();
        assert_eq!(tree.rows()[tree.selected()].name, "README.md");
        assert!(names(&tree).contains(&"a.txt".to_string()));
    }

    #[test]
    fn new_root_resets_expansion() {
        let (_dir, mut tree) = sample();
        tree.move_by(1);
        tree.toggle_selected();
        let other = tempfile::tempdir().unwrap();
        tree.set_root(other.path().to_path_buf());
        assert!(tree.rows().is_empty());
        assert_eq!(tree.selected(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_directory_is_not_expanded() {
        let (dir, mut tree) = sample();
        std::os::unix::fs::symlink(dir.path().join("src"), dir.path().join("link")).unwrap();
        tree.refresh();
        let link = tree.rows().iter().position(|r| r.name == "link").unwrap();
        assert_eq!(tree.rows()[link].kind, SandboxEntryKind::Symlink);
        tree.move_by(isize::try_from(link).unwrap());
        tree.toggle_selected();
        assert!(!tree.rows()[link].expanded);
        assert_eq!(names(&tree).len(), 5);
    }

    fn entry(name: &str, is_dir: bool) -> RemoteEntry {
        RemoteEntry {
            name: name.to_string(),
            is_dir,
        }
    }

    #[test]
    fn a_remote_tree_shows_what_it_is_sent_and_asks_for_what_it_opens() {
        let mut tree = SandboxTree::default();
        let root = PathBuf::from("/workspace");
        tree.set_remote_root(root.clone());
        assert!(tree.is_remote());
        assert_eq!(tree.unlisted(), Some(None));
        assert_eq!(tree.wanted(), std::slice::from_ref(&root));
        tree.asked(root.clone());
        assert!(tree.wanted().is_empty(), "asked about once");

        tree.set_listing(
            root.clone(),
            vec![
                entry("README.md", false),
                entry("src", true),
                entry("Docs", true),
            ],
        );
        assert_eq!(names(&tree), ["Docs", "src", "README.md"]);
        assert_eq!(tree.unlisted(), None);

        tree.move_by(1);
        tree.toggle_selected();
        let src = root.join("src");
        assert_eq!(tree.wanted(), std::slice::from_ref(&src));
        assert_eq!(names(&tree), ["Docs", "src", "README.md"]);
        tree.asked(src.clone());
        tree.set_listing(src.clone(), vec![entry("main.rs", false)]);
        assert_eq!(names(&tree), ["Docs", "src", "  main.rs", "README.md"]);
        assert_eq!(tree.rows()[2].path, src.join("main.rs"));

        tree.refresh();
        let mut again = tree.wanted();
        again.sort();
        assert_eq!(again, [root, src]);
        assert_eq!(
            names(&tree),
            ["Docs", "src", "  main.rs", "README.md"],
            "what shows stays until listed again"
        );
    }

    #[test]
    fn a_remote_root_that_could_not_be_listed_says_why_until_asked_again() {
        let mut tree = SandboxTree::default();
        let root = PathBuf::from("/workspace");
        tree.set_remote_root(root.clone());
        tree.asked(root.clone());
        tree.listing_failed(root.clone(), "asleep".to_string());
        assert_eq!(tree.unlisted(), Some(Some("asleep")));
        assert!(tree.wanted().is_empty(), "not asked again on its own");

        tree.refresh();
        assert_eq!(tree.wanted(), std::slice::from_ref(&root));
        tree.set_listing(root, Vec::new());
        assert_eq!(tree.unlisted(), None);
    }

    #[test]
    fn a_local_root_after_a_remote_one_reads_the_disk() {
        let (dir, mut tree) = sample();
        tree.set_remote_root(PathBuf::from("/workspace"));
        assert!(tree.rows().is_empty());
        tree.set_root(dir.path().to_path_buf());
        assert!(!tree.is_remote());
        assert_eq!(names(&tree), ["Docs", "src", ".env", "README.md"]);
        assert!(tree.wanted().is_empty());
    }

    #[test]
    fn caps_large_directories() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..MAX_ENTRIES_PER_DIR + 3 {
            fs::write(dir.path().join(format!("f{i:04}")), "").unwrap();
        }
        let mut tree = SandboxTree::default();
        tree.set_root(dir.path().to_path_buf());
        assert_eq!(tree.rows().len(), MAX_ENTRIES_PER_DIR + 1);
        assert_eq!(tree.rows().last().unwrap().kind, SandboxEntryKind::More(3));
    }
}
