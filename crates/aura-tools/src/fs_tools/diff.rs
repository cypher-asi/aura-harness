//! Line-diff helper for file-mutating tools.
//!
//! Centralises the `similar`-based line counting that `fs_write`,
//! `fs_edit`, and `fs_delete` use to populate
//! [`aura_core::ToolResult::line_diff`]. Keeping the call in one place
//! means there's a single source of truth for "how do we count lines"
//! — the agent loop just reads what the tool layer reports rather than
//! re-implementing the diff math against tool inputs.

use similar::{ChangeTag, TextDiff};

/// Count `(lines_added, lines_removed)` between two strings via a
/// line-wise diff.
///
/// Both counts are clamped to `u32` via `saturating_add`; realistic
/// file edits never approach 4B lines and the saturating arithmetic
/// avoids panicking on pathological inputs.
#[must_use]
pub fn count_line_diff(old_text: &str, new_text: &str) -> (u32, u32) {
    let diff = TextDiff::from_lines(old_text, new_text);
    let mut added: u32 = 0;
    let mut removed: u32 = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added = added.saturating_add(1),
            ChangeTag::Delete => removed = removed.saturating_add(1),
            ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

/// Count lines in a string (newline-terminated). Used by `fs_write`'s
/// "create new file" path (no pre-content to diff against) and by
/// `fs_delete` (no post-content to diff against).
///
/// Empty string maps to 0; trailing newline doesn't count as an extra
/// line — matches the convention `similar` uses inside [`count_line_diff`]
/// so the two helpers agree on what "one line" means.
#[must_use]
pub fn count_lines(text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }
    let mut count: u32 = 0;
    for _ in text.lines() {
        count = count.saturating_add(1);
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_line_diff_pure_insert() {
        assert_eq!(count_line_diff("a\nb\n", "a\nb\nc\nd\n"), (2, 0));
    }

    #[test]
    fn count_line_diff_pure_delete() {
        assert_eq!(count_line_diff("a\nb\nc\nd\n", "a\nb\n"), (0, 2));
    }

    #[test]
    fn count_line_diff_replace_one_line() {
        // similar models a one-line replacement as 1 insert + 1 delete.
        assert_eq!(count_line_diff("a\nold\nc\n", "a\nnew\nc\n"), (1, 1));
    }

    #[test]
    fn count_line_diff_identical_strings() {
        assert_eq!(count_line_diff("same\nlines\n", "same\nlines\n"), (0, 0));
    }

    #[test]
    fn count_line_diff_create_from_empty() {
        // Pre-content empty (file didn't exist, or completely overwritten
        // from empty) — every output line is an insert.
        assert_eq!(count_line_diff("", "a\nb\nc\n"), (3, 0));
    }

    #[test]
    fn count_line_diff_delete_to_empty() {
        // Inverse: every input line counts as removed.
        assert_eq!(count_line_diff("a\nb\nc\n", ""), (0, 3));
    }

    #[test]
    fn count_lines_empty_string_is_zero() {
        assert_eq!(count_lines(""), 0);
    }

    #[test]
    fn count_lines_trailing_newline_does_not_double_count() {
        assert_eq!(count_lines("a\nb\nc\n"), 3);
    }

    #[test]
    fn count_lines_no_trailing_newline() {
        assert_eq!(count_lines("a\nb\nc"), 3);
    }

    #[test]
    fn count_lines_single_line() {
        assert_eq!(count_lines("hello"), 1);
    }
}
