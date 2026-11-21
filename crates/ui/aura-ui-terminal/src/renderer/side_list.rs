//! Pieces shared by the tabs of the side panel (SANDBOX, PROCESSES, TRACE,
//! PLANS, TASKS), so every tab draws its empty state, its rows area and its key
//! hint the same way.
//!
//! Every tab follows the same rules: rows are drawn with
//! [`super::panels::list_row`], a status color only ever sits on a row's
//! icon, the name is `foreground` (or `muted` once the item is finished),
//! and the hint at the bottom reads `⏎ open  <extras>  ⇧←→ tabs`.

use super::panels::truncate_record_info;
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
/// Hint of the PROCESSES tab.
pub(super) const PROCESSES_HINT: &str = "⏎ open  o agent  k kill  l log  ⇧←→ tabs";
/// Hint of the TRACE tab.
pub(super) const TRACE_HINT: &str = "⏎ open  g graph  ⇧←→ tabs";
/// Hint of the PLANS tab.
pub(super) const PLANS_HINT: &str = "⏎ open  t task  ←→ fold  ⇧←→ tabs";
/// Hint of the TASKS tab.
pub(super) const TASKS_HINT: &str = "⏎ open  p plan  ←→ fold  ⇧←→ tabs";
/// Hint of the Kernel panel's MEMBERS tab.
pub(super) const MEMBERS_HINT: &str = "⏎ open  o chat  ⇧←→ tabs";

/// Whether a side-panel row is still in play. This alone decides white
/// versus gray text in every tab, so the tabs read the same way:
///
/// | tab       | `Active` (foreground)        | `Settled` (muted)            |
/// |-----------|------------------------------|------------------------------|
/// | SANDBOX   | files and folders            | dot entries, "more" rows     |
/// | PROCESSES | running, idle, paused        | exited, failed, killed       |
/// | TRACE     | steps still pending          | finished steps               |
/// | PLANS     | open plans and todos         | done plans, closed todos     |
/// | TASKS     | running and open tasks       | done, failed, stopped tasks  |
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

/// Draw a tab's empty state: muted lines, each indented two columns.
pub(super) fn render_empty(frame: &mut Frame, inner: Rect, message: &[&str], theme: &Theme) {
    let muted = Style::default().fg(theme.colors.muted);
    let lines: Vec<Line> = message
        .iter()
        .map(|text| {
            if text.is_empty() {
                Line::default()
            } else {
                Line::from(Span::styled(format!("  {text}"), muted))
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Draw the empty state of a tab whose `hidden` items, `what` it lists,
/// all belong to other agents of the project than the active one.
pub(super) fn render_hidden(
    frame: &mut Frame,
    inner: Rect,
    (what, hidden): (&str, usize),
    theme: &Theme,
) {
    // Two short lines, so a narrow panel does not cut them off.
    let message = [
        format!("No {what} for this agent."),
        format!("The {} button lists", SideScope::BUTTON),
        format!("the project's {hidden}."),
    ];
    render_empty(frame, inner, &message.each_ref().map(String::as_str), theme);
}

/// Split a tab's body into its rows and, while the panel has focus and
/// there is room, a hint row at the bottom.
pub(super) fn split_hint(inner: Rect, focused: bool) -> (Rect, Option<Rect>) {
    if !focused || inner.height < 4 {
        return (inner, None);
    }
    let rows = Rect {
        height: inner.height - 1,
        ..inner
    };
    let hint = Rect {
        y: inner.y + inner.height - 1,
        height: 1,
        ..inner
    };
    (rows, Some(hint))
}

/// Draw `text` as the muted hint in `area`, cut to fit.
pub(super) fn render_hint(frame: &mut Frame, area: Rect, text: &str, theme: &Theme) {
    let hint = truncate_record_info(text, usize::from(area.width).saturating_sub(1));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {hint}"),
            Style::default().fg(theme.colors.muted),
        ))),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_share_one_format() {
        for hint in [
            SANDBOX_HINT,
            PROCESSES_HINT,
            TRACE_HINT,
            PLANS_HINT,
            TASKS_HINT,
            MEMBERS_HINT,
        ] {
            assert!(hint.starts_with("⏎ open  "), "{hint}");
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
    fn the_hint_row_needs_focus_and_room() {
        let inner = Rect::new(0, 0, 20, 10);
        let (rows, hint) = split_hint(inner, true);
        assert_eq!(rows.height, 9);
        assert_eq!(hint.map(|h| h.y), Some(9));
        assert_eq!(split_hint(inner, false), (inner, None));
        let short = Rect::new(0, 0, 20, 3);
        assert_eq!(split_hint(short, true), (short, None));
    }
}
