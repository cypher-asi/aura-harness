//! What follows the conversation while a turn is in flight: the to-do
//! checklist and the working line.

use crate::{app::AppState, App, Theme};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// What follows the conversation while a turn is in flight: the to-do
/// checklist, then the working line.
#[derive(Default)]
pub(super) struct LiveTail {
    pub(super) lines: Vec<Line<'static>>,
    /// Rows at the top of `lines` taken by the to-do checklist, each of
    /// which expands or collapses it when clicked.
    pub(super) checklist_rows: usize,
    /// Whether the checklist's last row is `N more` or `show less`.
    checklist_footer: bool,
    /// Row in `lines` of the working line, which opens the turn's records
    /// when clicked.
    pub(super) status_row: Option<usize>,
}

impl LiveTail {
    /// Whether `row` is one the mouse lights up over the checklist: the
    /// header and the `N more` or `show less` under the items, which say
    /// what a click does.
    pub(super) fn is_checklist_label(&self, row: usize) -> bool {
        (row == 0 && self.checklist_rows > 0)
            || (self.checklist_footer && row + 1 == self.checklist_rows)
    }
}

pub(super) fn live_tail(app: &App, width: usize, theme: &Theme) -> LiveTail {
    // A list with every item closed is finished: it stops showing even
    // while the turn goes on, and shows again once new items open.
    let checklist = if app.state() == AppState::Processing && app.hud().has_open_todos() {
        let frame = theme.animate_spinners.then(|| app.animation_frame());
        super::super::hud::todo_checklist(app.hud(), width, theme, frame, app.todos_expanded())
    } else {
        super::super::hud::TodoChecklist::default()
    };
    let checklist_rows = checklist.lines.len();
    let mut lines = checklist.lines;
    let status_row = working_indicator_line(app, theme).map(|line| {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(line);
        lines.len() - 1
    });
    LiveTail {
        lines,
        checklist_rows,
        checklist_footer: checklist.has_footer,
        status_row,
    }
}

/// `⬢ Thinking… 12s` — shown below the conversation, flush with the
/// prose, while a turn is in flight and no reply text is streaming; a
/// running tool call counts its own time, and a command that has printed
/// nothing for a while says so (`· no output 45s`). It
/// stays the last line even while a group of steps is still growing above
/// it, and a band of light sweeps across its label to show work goes on.
pub(super) fn working_indicator_line<'a>(app: &App, theme: &Theme) -> Option<Line<'a>> {
    let label = format!("{}…", capitalized(&app.working_label()?));
    let label_style = Style::default().add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::styled(
        "⬢ ",
        Style::default().fg(theme.colors.success),
    )];
    if theme.animate_spinners {
        spans.extend(crate::animation::shimmer_spans(
            &label,
            app.animation_frame(),
            label_style,
            (theme.colors.muted, theme.colors.foreground),
        ));
    } else {
        spans.push(Span::styled(label, label_style.fg(theme.colors.foreground)));
    }
    let (elapsed, quiet) = app.working_times();
    if let Some(elapsed) = elapsed.filter(|e| e.as_secs() > 0) {
        spans.push(Span::styled(
            format!(" {}", format_elapsed(elapsed)),
            Style::default().fg(theme.colors.muted),
        ));
    }
    if let Some(quiet) = quiet {
        spans.push(Span::styled(
            format!(" · no output {}", format_elapsed(quiet)),
            Style::default().fg(theme.colors.success),
        ));
    }
    Some(Line::from(spans))
}

/// `text` with its first letter in upper case.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `7s`, `2m 05s`.
pub(crate) fn format_elapsed(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}
