//! Input history management.

use std::collections::VecDeque;

/// Maximum history entries to keep.
const MAX_HISTORY: usize = 100;

/// Input history for command recall.
#[derive(Debug, Clone)]
pub struct InputHistory {
    /// History entries (newest first)
    entries: VecDeque<String>,
    /// Current position in history (None = not browsing, Some(i) = at index i)
    position: Option<usize>,
}

impl InputHistory {
    /// Create a new empty history.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            position: None,
        }
    }

    /// Add an entry to history.
    pub fn add(&mut self, entry: &str) {
        let entry = entry.trim().to_string();
        if entry.is_empty() {
            return;
        }

        // Don't add duplicates of the most recent entry
        if self.entries.front() == Some(&entry) {
            return;
        }

        self.entries.push_front(entry);
        while self.entries.len() > MAX_HISTORY {
            self.entries.pop_back();
        }

        // Reset position when adding new entry
        self.position = None;
    }

    /// Get the previous entry (older).
    #[must_use]
    pub fn previous(&mut self) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }

        let next_pos = match self.position {
            None => 0,
            Some(pos) if pos + 1 < self.entries.len() => pos + 1,
            Some(pos) => pos, // Stay at oldest
        };

        self.position = Some(next_pos);
        self.entries.get(next_pos).map(String::as_str)
    }

    /// Get the next entry (newer), returning None when back at current input.
    #[must_use]
    pub fn next_newer(&mut self) -> Option<&str> {
        match self.position {
            None | Some(0) => {
                self.position = None;
                None
            }
            Some(pos) => {
                let new_pos = pos - 1;
                self.position = Some(new_pos);
                self.entries.get(new_pos).map(String::as_str)
            }
        }
    }

    /// Reset the browsing position.
    pub fn reset(&mut self) {
        self.position = None;
    }

    /// Get the number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if history is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get all entries (newest first).
    pub fn entries(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(String::as_str)
    }
}

impl Default for InputHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_add() {
        let mut history = InputHistory::new();
        history.add("first");
        history.add("second");
        history.add("third");

        assert_eq!(history.len(), 3);
    }

    #[test]
    fn test_history_navigation() {
        let mut history = InputHistory::new();
        history.add("first");
        history.add("second");
        history.add("third");

        // Go back
        assert_eq!(history.previous(), Some("third"));
        assert_eq!(history.previous(), Some("second"));
        assert_eq!(history.previous(), Some("first"));
        assert_eq!(history.previous(), Some("first")); // Stay at oldest

        // Go forward
        assert_eq!(history.next_newer(), Some("second"));
        assert_eq!(history.next_newer(), Some("third"));
        assert_eq!(history.next_newer(), None); // Back to current
    }

    #[test]
    fn test_history_no_duplicates() {
        let mut history = InputHistory::new();
        history.add("same");
        history.add("same");
        history.add("same");

        assert_eq!(history.len(), 1);
    }

    #[test]
    fn test_history_empty_entries() {
        let mut history = InputHistory::new();
        history.add("");
        history.add("   ");

        assert!(history.is_empty());
    }

    #[test]
    fn test_history_reset() {
        let mut history = InputHistory::new();
        history.add("first");
        history.add("second");

        let _ = history.previous();
        let _ = history.previous();
        history.reset();

        assert_eq!(history.previous(), Some("second"));
    }
}
