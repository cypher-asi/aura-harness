//! Todo checkbox helpers, shared by the TASKS tab's plan todos and the
//! to-do checklist that follows the conversation.
//!
//! ```text
//!  ✓  Add session table
//!  ◐  Write the login handler
//!  ☐  Wire the middleware
//!  ⊘  Drop the old cookie
//! ```
//!
//! Each state has its own shape, not only its own fill, so an item in
//! progress never reads as done: a box waits, a half-filled circle is
//! under way, a check is done and a barred circle was dropped.

use crate::events::TodoState;
use crate::Theme;
use ratatui::style::{Color, Modifier, Style};

/// A todo's marker: an empty box while it waits, a half-filled circle
/// while in progress, a check when done, a barred circle when dropped.
pub(super) const fn marker(status: TodoState) -> &'static str {
    match status {
        TodoState::Pending => "☐",
        TodoState::InProgress => "◐",
        TodoState::Completed => "✓",
        TodoState::Cancelled => "⊘",
    }
}

/// Color of a todo's box: closed todos fade, the one in progress stands
/// out, pending ones wait in gray.
pub(super) const fn marker_color(state: TodoState, theme: &Theme) -> Color {
    match state {
        TodoState::InProgress => theme.colors.foreground,
        TodoState::Pending => theme.colors.muted,
        TodoState::Completed | TodoState::Cancelled => theme.colors.faint,
    }
}

/// How a todo's text is drawn, everywhere a todo shows: in its box's
/// color, bold while in progress, struck through when cancelled.
pub(super) fn todo_style(state: TodoState, theme: &Theme) -> Style {
    let style = Style::default().fg(marker_color(state, theme));
    match state {
        TodoState::InProgress => style.add_modifier(Modifier::BOLD),
        TodoState::Cancelled => style.add_modifier(Modifier::CROSSED_OUT),
        TodoState::Pending | TodoState::Completed => style,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [TodoState; 4] = [
        TodoState::Pending,
        TodoState::InProgress,
        TodoState::Completed,
        TodoState::Cancelled,
    ];

    #[test]
    fn every_state_has_a_marker_of_its_own() {
        let markers: Vec<&str> = STATES.into_iter().map(marker).collect();
        let mut distinct = markers.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), STATES.len(), "{markers:?}");
    }

    #[test]
    fn an_item_in_progress_never_looks_done_or_filled() {
        let in_progress = marker(TodoState::InProgress);
        // The filled and crossed boxes read as checked.
        for checked in ["■", "▣", "☒", "☑", "✓", "✔"] {
            assert_ne!(in_progress, checked);
        }
        assert_eq!(marker(TodoState::Completed), "✓");
        assert_eq!(marker(TodoState::Pending), "☐");
    }
}
