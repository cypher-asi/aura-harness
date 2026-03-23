//! Read guard — limits redundant file re-reads.

use std::collections::HashMap;

/// Tracks per-file read counts to prevent re-reading.
#[derive(Debug, Default, Clone)]
pub struct ReadGuardState {
    /// Full-file read counts per path.
    full_reads: HashMap<String, usize>,
    /// Range-read counts per path.
    range_reads: HashMap<String, usize>,
}

impl ReadGuardState {
    /// Record a full file read.
    pub(crate) fn record_full_read(&mut self, path: &str) {
        *self.full_reads.entry(path.to_string()).or_insert(0) += 1;
    }

    /// Record a range (partial) file read.
    pub(crate) fn record_range_read(&mut self, path: &str) {
        *self.range_reads.entry(path.to_string()).or_insert(0) += 1;
    }

    /// Get the full-read count for a path.
    #[must_use]
    pub fn full_read_count(&self, path: &str) -> usize {
        self.full_reads.get(path).copied().unwrap_or(0)
    }

    /// Get the range-read count for a path.
    #[must_use]
    pub fn range_read_count(&self, path: &str) -> usize {
        self.range_reads.get(path).copied().unwrap_or(0)
    }

    /// Reset read counts for a path (called after a successful write).
    pub fn reset_for_path(&mut self, path: &str) {
        self.full_reads.remove(path);
        self.range_reads.remove(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_read_allowed_up_to_limit() {
        let mut guard = ReadGuardState::default();
        guard.record_full_read("test.rs");
        guard.record_full_read("test.rs");
        assert_eq!(guard.full_read_count("test.rs"), 2);
    }

    #[test]
    fn test_range_read_tracked_separately() {
        let mut guard = ReadGuardState::default();
        guard.record_full_read("test.rs");
        guard.record_range_read("test.rs");
        assert_eq!(guard.full_read_count("test.rs"), 1);
        assert_eq!(guard.range_read_count("test.rs"), 1);
    }

    #[test]
    fn test_reset_clears_both() {
        let mut guard = ReadGuardState::default();
        guard.record_full_read("test.rs");
        guard.record_range_read("test.rs");
        guard.reset_for_path("test.rs");
        assert_eq!(guard.full_read_count("test.rs"), 0);
        assert_eq!(guard.range_read_count("test.rs"), 0);
    }
}
