//! Changelog fragments: one file per change in `changelog.d/`, put into
//! `CHANGELOG.md` at release time.
//!
//! Every change used to add its entry at the top of `CHANGELOG.md`'s
//! `[Unreleased]` section, so nearly every pair of parallel branches
//! conflicted there. Now a change adds `changelog.d/<slug>.md` instead
//! ([`fragment_name`]), a file no other branch writes, and a release
//! puts the fragments under `## [Unreleased]` ([`assemble`]) and deletes
//! them (`aura changelog --write`).
//!
//! A fragment holds what an entry always held:
//!
//! ```markdown
//! ### What changed, as a title
//!
//! ##### Fixed
//!
//! - What the user sees now.
//! ```
//!
//! ## Invariants
//!
//! - Assembly is pure and keeps the changelog's text outside the new
//!   block byte for byte; the fragments go in the order given (the
//!   caller puts the newest first).
//! - `README.md` in the folder is not a fragment, nor is any file that
//!   doesn't end in `.md`.
//!
//! ## Failure modes
//!
//! A changelog with no `## [Unreleased]` heading, an empty fragment, or
//! one that doesn't start with a `### ` title fails assembly with the
//! fragment's name; nothing is written then.

use std::path::{Path, PathBuf};
use thiserror::Error;

/// The folder fragments live in, relative to the repo's top.
pub const FRAGMENT_DIR: &str = "changelog.d";
/// The changelog fragments are assembled into, relative to the repo's top.
pub const CHANGELOG_FILE: &str = "CHANGELOG.md";
/// The heading fragments go under.
pub const UNRELEASED: &str = "## [Unreleased]";
/// The file in [`FRAGMENT_DIR`] that explains it, never a fragment.
pub const README: &str = "README.md";

/// Why fragments could not be read or assembled.
#[derive(Debug, Error)]
pub enum ChangelogError {
    /// A file or folder couldn't be read.
    #[error("{path}: {source}")]
    Io {
        /// What was being read
        path: String,
        /// The underlying error
        source: std::io::Error,
    },
    /// The changelog has no `## [Unreleased]` heading to put them under.
    #[error("{CHANGELOG_FILE} has no `{UNRELEASED}` heading")]
    NoUnreleased,
    /// A fragment is empty.
    #[error("changelog fragment {0} is empty")]
    Empty(String),
    /// A fragment doesn't start with a `### ` title.
    #[error("changelog fragment {0} must start with a `### ` title")]
    NoTitle(String),
}

/// One fragment: its file name in [`FRAGMENT_DIR`] and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    /// File name, such as `fewer-merge-conflicts-rus204.md`
    pub name: String,
    /// Its contents
    pub text: String,
}

impl Fragment {
    /// The fragments in `dir`, by file name; none when `dir` doesn't
    /// exist.
    ///
    /// # Errors
    /// `dir` or a fragment in it can't be read.
    pub fn read_dir(dir: &Path) -> Result<Vec<Self>, ChangelogError> {
        let listed = match std::fs::read_dir(dir) {
            Ok(listed) => listed,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io(dir, e)),
        };
        let mut paths = Vec::new();
        for item in listed {
            let path = item.map_err(|e| io(dir, e))?.path();
            if path.is_file() && is_fragment_name(&path) {
                paths.push(path);
            }
        }
        paths.sort();
        paths
            .into_iter()
            .map(|path| {
                let text = std::fs::read_to_string(&path).map_err(|e| io(&path, e))?;
                Ok(Self {
                    name: file_name(&path),
                    text,
                })
            })
            .collect()
    }

    /// The fragment's text, checked: trimmed, and starting with a title.
    ///
    /// # Errors
    /// It is empty or has no `### ` title first.
    pub fn block(&self) -> Result<&str, ChangelogError> {
        let text = self.text.trim();
        if text.is_empty() {
            return Err(ChangelogError::Empty(self.name.clone()));
        }
        if !text.starts_with("### ") {
            return Err(ChangelogError::NoTitle(self.name.clone()));
        }
        Ok(text)
    }
}

/// Whether `path` names a fragment: a `.md` file other than the README.
#[must_use]
pub fn is_fragment_name(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "md") && file_name(path) != README
}

/// Whether `path`, relative to the repo's top, is a fragment.
#[must_use]
pub fn is_fragment_path(path: &str) -> bool {
    let path = Path::new(path);
    path.parent() == Some(Path::new(FRAGMENT_DIR)) && is_fragment_name(path)
}

/// The fragment file a change on `branch` adds: the branch's last path
/// segment, with anything but letters, digits, `-`, `_` and `.` turned
/// into `-`, then `.md` (`aura/fix-x-rus1` gives `fix-x-rus1.md`).
#[must_use]
pub fn fragment_name(branch: &str) -> String {
    let last = branch.rsplit('/').next().unwrap_or(branch);
    let slug: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches(|c| c == '-' || c == '.');
    if slug.is_empty() {
        "change.md".to_string()
    } else {
        format!("{slug}.md")
    }
}

/// The block the fragments make, in the order given, separated by blank
/// lines; empty for no fragments.
///
/// # Errors
/// A fragment is empty or has no title.
pub fn section(fragments: &[Fragment]) -> Result<String, ChangelogError> {
    let blocks = fragments
        .iter()
        .map(Fragment::block)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(blocks.join("\n\n"))
}

/// `changelog` with the fragments, in the order given, at the top of its
/// `[Unreleased]` section. Unchanged when there are none.
///
/// # Errors
/// There is no `## [Unreleased]` heading, or a fragment is empty or has
/// no title.
pub fn assemble(changelog: &str, fragments: &[Fragment]) -> Result<String, ChangelogError> {
    let block = section(fragments)?;
    let (before, after) = split_at_unreleased(changelog)?;
    if block.is_empty() {
        return Ok(changelog.to_string());
    }
    let rest = after.trim_start_matches(['\n', '\r']);
    let mut out = String::with_capacity(changelog.len() + block.len() + 4);
    out.push_str(before);
    out.push_str("\n\n");
    out.push_str(&block);
    out.push('\n');
    if !rest.is_empty() {
        out.push('\n');
        out.push_str(rest);
    }
    Ok(out)
}

/// `changelog` cut at the end of its `## [Unreleased]` heading line.
fn split_at_unreleased(changelog: &str) -> Result<(&str, &str), ChangelogError> {
    let mut offset = 0;
    for line in changelog.split_inclusive('\n') {
        let end = offset + line.trim_end_matches(['\n', '\r']).len();
        if line.trim_end() == UNRELEASED {
            return Ok((&changelog[..end], &changelog[end..]));
        }
        offset += line.len();
    }
    Err(ChangelogError::NoUnreleased)
}

/// `fragments` ordered newest first by `added`, each fragment's name
/// with when git added it; a fragment it doesn't have (not committed yet)
/// counts as newest. Ties keep name order.
#[must_use]
pub fn newest_first(
    mut fragments: Vec<Fragment>,
    added: &std::collections::HashMap<String, i64>,
) -> Vec<Fragment> {
    fragments.sort_by_key(|fragment| {
        std::cmp::Reverse(added.get(&fragment.name).copied().unwrap_or(i64::MAX))
    });
    fragments
}

/// The paths of `fragments` in `dir`.
#[must_use]
pub fn paths_in(dir: &Path, fragments: &[Fragment]) -> Vec<PathBuf> {
    fragments
        .iter()
        .map(|fragment| dir.join(&fragment.name))
        .collect()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn io(path: &Path, source: std::io::Error) -> ChangelogError {
    ChangelogError::Io {
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests;
