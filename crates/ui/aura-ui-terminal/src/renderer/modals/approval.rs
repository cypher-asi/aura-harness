//! The approval: what a tool wants to do, and the answers.

use super::super::text::wrap_words;
use super::with_notice;
use crate::app::PendingApproval;
use crate::components::{choice_line, Modal, ModalArea, Tone};
use crate::{App, Theme};
use ratatui::{
    style::Style,
    text::{Line, Span},
    Frame,
};

/// Most lines of the action's description shown.
const MAX_DESCRIPTION_LINES: usize = 6;

/// Draw the first call waiting for an answer, with how many wait when
/// more than one does.
pub(super) fn render(frame: &mut Frame, app: &App, theme: &Theme) -> Option<ModalArea> {
    let approval = app.pending_approval()?;
    let screen = frame.area();
    let mut modal = Modal::new("APPROVAL REQUIRED", theme).tone(Tone::Attention);
    let waiting = app.pending_approval_count();
    if waiting > 1 {
        modal = modal.meta(format!("1 of {waiting}"));
    }
    let modal = with_notice(modal, app, theme);
    let lines = approval_lines(approval, modal.body_width(screen), theme);
    Some(modal.render(frame.buffer_mut(), screen, lines))
}

/// The tool, its description cut to a few lines, and the answers.
fn approval_lines(approval: &PendingApproval, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let muted = Style::default().fg(theme.colors.muted);
    let mut lines = vec![Line::from(Span::styled(
        format!("{} wants to:", approval.tool),
        Style::default().fg(theme.colors.foreground),
    ))];
    let description = wrap_words(&approval.description, width.max(1));
    let cut = description.len() > MAX_DESCRIPTION_LINES;
    lines.extend(
        description
            .into_iter()
            .take(MAX_DESCRIPTION_LINES)
            .map(|line| Line::from(Span::styled(line, muted))),
    );
    if cut {
        lines.push(Line::from(Span::styled("…", muted)));
    }
    lines.push(Line::default());
    lines.push(choice_line('y', "Allow once", theme.colors.success));
    if !approval.always.is_empty() {
        let always = format!("Always allow {}", approval.always);
        lines.push(choice_line('a', &always, theme.colors.primary));
    }
    lines.push(choice_line('n', "Deny", theme.colors.error));
    lines
}
