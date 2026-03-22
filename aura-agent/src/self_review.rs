//! Self-review guard — tracks modified vs read paths and requires
//! re-reading before task completion.

use std::collections::HashSet;

/// Normalise a tool-reported path so that backslashes and leading `./`
/// don't cause mismatches.
pub fn normalize_tool_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}

/// Tracks which files have been written and which have been re-read
/// since their last write.  When a task attempts to complete, the guard
/// can report any modified files that haven't been reviewed yet.
///
/// The guard fires **at most once** — after it reports unreviewed files
/// it won't block subsequent completion attempts.
#[derive(Debug, Clone)]
pub struct SelfReviewGuard {
    modified: HashSet<String>,
    read_since_write: HashSet<String>,
    prompted: bool,
}

impl Default for SelfReviewGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl SelfReviewGuard {
    #[must_use]
    pub fn new() -> Self {
        Self {
            modified: HashSet::new(),
            read_since_write: HashSet::new(),
            prompted: false,
        }
    }

    /// Record that `path` was written (create / modify / search-replace).
    /// Invalidates any previous read for the same path.
    pub fn record_write(&mut self, path: &str) {
        let norm = normalize_tool_path(path);
        self.modified.insert(norm.clone());
        self.read_since_write.remove(&norm);
    }

    /// Record that `path` was read.
    pub fn record_read(&mut self, path: &str) {
        self.read_since_write
            .insert(normalize_tool_path(path));
    }

    /// Returns the list of modified-but-unreviewed paths, or `None` if
    /// all modified files have been re-read (or no files were modified,
    /// or the guard already prompted once).
    pub fn check_review_needed(&mut self) -> Option<Vec<String>> {
        if self.prompted || self.modified.is_empty() {
            return None;
        }
        let mut unreviewed: Vec<String> = self
            .modified
            .iter()
            .filter(|p| !self.read_since_write.contains(p.as_str()))
            .cloned()
            .collect();
        if unreviewed.is_empty() {
            return None;
        }
        unreviewed.sort();
        self.prompted = true;
        Some(unreviewed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_modified_files_returns_none() {
        let mut guard = SelfReviewGuard::new();
        assert!(guard.check_review_needed().is_none());
    }

    #[test]
    fn unreviewed_write_reported() {
        let mut guard = SelfReviewGuard::new();
        guard.record_write("src/main.rs");
        let unreviewed = guard.check_review_needed().unwrap();
        assert_eq!(unreviewed, vec!["src/main.rs"]);
    }

    #[test]
    fn read_after_write_clears_review() {
        let mut guard = SelfReviewGuard::new();
        guard.record_write("src/main.rs");
        guard.record_read("src/main.rs");
        assert!(guard.check_review_needed().is_none());
    }

    #[test]
    fn write_after_read_invalidates() {
        let mut guard = SelfReviewGuard::new();
        guard.record_write("src/main.rs");
        guard.record_read("src/main.rs");
        guard.record_write("src/main.rs");
        let unreviewed = guard.check_review_needed().unwrap();
        assert_eq!(unreviewed, vec!["src/main.rs"]);
    }

    #[test]
    fn fires_only_once() {
        let mut guard = SelfReviewGuard::new();
        guard.record_write("src/main.rs");
        assert!(guard.check_review_needed().is_some());
        assert!(guard.check_review_needed().is_none());
    }

    #[test]
    fn normalizes_backslashes_and_dot_prefix() {
        let mut guard = SelfReviewGuard::new();
        guard.record_write(r".\src\main.rs");
        guard.record_read("src/main.rs");
        assert!(guard.check_review_needed().is_none());
    }
}
