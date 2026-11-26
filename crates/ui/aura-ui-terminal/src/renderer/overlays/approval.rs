//! The approval modal: what a tool wants to do and the keys that answer.

use super::super::text::{centered_rect, wrap_words};
use crate::Theme;
use ratatui::{
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

/// Render the approval modal.
pub(super) fn render_approval_modal(
    frame: &mut Frame,
    approval: &crate::app::PendingApproval,
    waiting: usize,
    theme: &Theme,
) {
    let area = frame.area();
    let modal_width = 64.min(area.width.saturating_sub(4));
    let text_width = usize::from(modal_width.saturating_sub(2)).max(1);
    let content = approval_lines(approval, text_width, theme);

    let height = u16::try_from(content.len() + 2).unwrap_or(u16::MAX);
    let modal_area = centered_rect(modal_width, height.min(area.height), area);
    frame.render_widget(Clear, modal_area);

    let title = if waiting > 1 {
        format!(" Approval Required (1 of {waiting}) ")
    } else {
        " Approval Required ".to_string()
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.warning));
    let paragraph = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, modal_area);
}

/// The modal's lines: the tool, its description cut to a few lines, and
/// the answers.
fn approval_lines(
    approval: &crate::app::PendingApproval,
    text_width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    /// Most lines of the action's description shown.
    const MAX_DESCRIPTION_LINES: usize = 6;

    let muted = Style::default().fg(theme.colors.muted);
    let mut content = vec![Line::from(Span::styled(
        format!("{} wants to:", approval.tool),
        Style::default().fg(theme.colors.foreground),
    ))];
    let description = wrap_words(&approval.description, text_width);
    let cut = description.len() > MAX_DESCRIPTION_LINES;
    content.extend(
        description
            .into_iter()
            .take(MAX_DESCRIPTION_LINES)
            .map(|line| Line::from(Span::styled(line, muted))),
    );
    if cut {
        content.push(Line::from(Span::styled("…", muted)));
    }
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "[Y] Allow once",
        Style::default().fg(theme.colors.success),
    )));
    if !approval.always.is_empty() {
        content.push(Line::from(Span::styled(
            format!("[A] Always allow {}", approval.always),
            Style::default().fg(theme.colors.primary),
        )));
    }
    content.push(Line::from(Span::styled(
        "[N] Deny (Esc)",
        Style::default().fg(theme.colors.error),
    )));
    content
}
