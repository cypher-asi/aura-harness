//! Pieces shared by the tabs of the side and Kernel panels, so every tab
//! draws its empty state, its key hint and its text tone the same way.
//! Each tab draws into a [`super::chrome::Body`], which puts the hint on
//! its last row while the panel has focus.
//!
//! List tabs follow the same rules: rows are drawn with
//! [`crate::components::list_row`], a status color only ever sits on a
//! row's icon, the name is `foreground` (or `muted` once the item is
//! finished), and the hint reads `⏎ open  <extras>  ⇧←→ tabs`. Info tabs
//! are [`crate::components::Info`] documents with [`INFO_HINT`].

use crate::components::truncate_record_info;
use crate::{SideScope, Theme};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Hint of the SANDBOX tab.
pub(super) const SANDBOX_HINT: &str = "⏎ open  ←→ fold  ⇧←→ tabs";
/// Hint of the REPO tab, and of it on a PR, which has a page to open.
pub(super) const REPO_HINT: &str = "⏎ open  [] filter  r refresh  ⇧←→ tabs";
pub(super) const REPO_PR_HINT: &str = "⏎ open  o web  [] filter  ⇧←→ tabs";
/// Hint of the PROCESSES tab.
pub(super) const PROCESSES_HINT: &str = "⏎ open  o agent  k kill  l log  ⇧←→ tabs";
/// Hint of the TRACE tab.
pub(super) const TRACE_HINT: &str = "⏎ open  g graph  ⇧←→ tabs";
/// Hint of the TASKS tab, which also lists the plans the tasks work on.
pub(super) const TASKS_HINT: &str = "⏎ open  p plan  t task  ←→ fold  ⇧←→ tabs";
/// Hint of an open channel's MEMBERS tab, whose rows only show.
pub(super) const CHANNEL_MEMBERS_HINT: &str = "↑↓ move  ⇧←→ tabs";
/// Hint of the Kernel panel's KERNEL tab, its records.
pub(super) const KERNEL_HINT: &str = "⏎ open  ⇧←→ tabs";
/// Hint of the Kernel panel's MEMBERS tab.
pub(super) const MEMBERS_HINT: &str = "⏎ open  o chat  ⇧←→ tabs";
/// Hint of every info tab (POLICIES, HOST, a channel's INFO): a document
/// with nothing to open.
pub(super) const INFO_HINT: &str = "↑↓ scroll  ⇧←→ tabs";
/// Hint of the Kernel panel's INBOX tab, whose threads also fold.
pub(super) const INBOX_HINT: &str = "↑↓ move  ←→ fold  ⏎ open  ⇧←→ tabs";

/// Whether a side-panel row is still in play. This alone decides white
/// versus gray text in every tab, so the tabs read the same way:
///
/// | tab       | `Active` (foreground)                    | `Settled` (muted)                            |
/// |-----------|------------------------------------------|----------------------------------------------|
/// | SANDBOX   | files and folders                        | dot entries, "more" rows                     |
/// | REPO      | changes, commits, open PRs, branches     | merged and closed PRs, gone branches, notes  |
/// | PROCESSES | running, idle, paused                    | exited, failed, killed                       |
/// | TRACE     | steps still pending                      | finished steps                               |
/// | TASKS     | open plans and todos, running/open tasks | done plans, closed todos, done/failed tasks  |
///
/// Status (ok, warning, error) is shown by the row's icon color only, never
/// by its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    Active,
    Settled,
}

impl Tone {
    pub(super) const fn active_if(active: bool) -> Self {
        if active {
            Self::Active
        } else {
            Self::Settled
        }
    }
}

/// The name style of a row with `tone`.
pub(super) fn name_style(tone: Tone, theme: &Theme) -> Style {
    Style::default().fg(match tone {
        Tone::Active => theme.colors.foreground,
        Tone::Settled => theme.colors.muted,
    })
}

/// Columns kept clear on each side of a panel's empty message.
pub(super) const EMPTY_INDENT: usize = 2;

/// A tab's empty state: one muted line, indented two columns, cut to
/// `width` with an ellipsis rather than wrapped. Lists that build their
/// own lines use this too, so every empty state reads the same way.
pub(super) fn empty_line(text: &str, width: u16, theme: &Theme) -> Line<'static> {
    let room = usize::from(width).saturating_sub(EMPTY_INDENT);
    let text = truncate_record_info(text, room);
    Line::from(Span::styled(
        format!("{}{text}", " ".repeat(EMPTY_INDENT)),
        Style::default().fg(theme.colors.muted),
    ))
}

/// Draw a tab's empty state: a single line, never wrapped.
pub(super) fn render_empty(frame: &mut Frame, inner: Rect, text: &str, theme: &Theme) {
    frame.render_widget(Paragraph::new(empty_line(text, inner.width, theme)), inner);
}

/// Draw the empty state of a tab whose `hidden` items, `what` it lists,
/// all belong to other agents of the project than the active one.
pub(super) fn render_hidden(
    frame: &mut Frame,
    inner: Rect,
    (what, hidden): (&str, usize),
    theme: &Theme,
) {
    let message = format!("No {what} for this agent. {} {hidden}.", SideScope::BUTTON);
    render_empty(frame, inner, &message, theme);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_share_one_format() {
        for hint in [
            SANDBOX_HINT,
            REPO_HINT,
            REPO_PR_HINT,
            PROCESSES_HINT,
            TRACE_HINT,
            TASKS_HINT,
            KERNEL_HINT,
            MEMBERS_HINT,
        ] {
            assert!(hint.starts_with("⏎ open  "), "{hint}");
            assert!(hint.ends_with("  ⇧←→ tabs"), "{hint}");
        }
        for hint in [INFO_HINT, CHANNEL_MEMBERS_HINT, INBOX_HINT] {
            assert!(hint.ends_with("  ⇧←→ tabs"), "{hint}");
        }
    }

    #[test]
    fn the_tone_rule_is_the_only_white_or_gray_switch() {
        let theme = Theme::cyber();
        assert_eq!(
            name_style(Tone::Active, &theme).fg,
            Some(theme.colors.foreground)
        );
        assert_eq!(
            name_style(Tone::Settled, &theme).fg,
            Some(theme.colors.muted)
        );
        assert_eq!(Tone::active_if(true), Tone::Active);
        assert_eq!(Tone::active_if(false), Tone::Settled);
    }

    #[test]
    fn an_empty_message_is_one_indented_line_even_when_narrow() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let theme = Theme::cyber();
        let mut terminal = Terminal::new(TestBackend::new(12, 3)).expect("test terminal");
        terminal
            .draw(|frame| {
                render_empty(
                    frame,
                    frame.area(),
                    "No messages between agents yet.",
                    &theme,
                );
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let rows: Vec<String> = (0..3)
            .map(|y| (0..12).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        assert_eq!(rows[0], "  No messag…", "{rows:?}");
        assert!(rows[1].trim().is_empty(), "{rows:?}");
        assert!(rows[2].trim().is_empty(), "{rows:?}");
    }
}
