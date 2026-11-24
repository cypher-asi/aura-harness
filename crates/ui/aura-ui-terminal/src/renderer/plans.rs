//! Todo checkbox helpers, shared by the TASKS tab's plan todos and the
//! to-do checklist that follows the conversation.
//!
//! ```text
//!  ☒  Add session table
//!  ▣  Write the login handler
//!  ☐  Wire the middleware
//! ```

use crate::events::TodoState;
use crate::Theme;
use ratatui::style::{Color, Modifier, Style};

/// A todo's box, as in the to-do checklist that follows the conversation:
/// crossed when done, filled while in progress, empty otherwise.
pub(super) const fn marker(status: TodoState) -> &'static str {
    match status {
        TodoState::Pending | TodoState::Cancelled => "☐",
        TodoState::InProgress => "▣",
        TodoState::Completed => "☒",
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
