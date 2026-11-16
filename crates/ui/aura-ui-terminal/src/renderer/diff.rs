//! File changes in the chat: the `/diff` message, one diff per file.

use super::segments::{message_style, prefixed_lines};
use super::text::Measure;
use crate::components::{DiffBlock, DiffView, MessageRole};
use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// Diff lines shown in one message before the rest is cut.
const MAX_DIFF_LINES: usize = 400;

/// `+3 -1` for a file header or a tool block.
pub(super) fn diff_stat_spans(view: &DiffView, theme: &Theme) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            format!("+{}", view.additions()),
            Style::default().fg(theme.colors.success),
        ),
        Span::raw(" "),
        Span::styled(
            format!("-{}", view.deletions()),
            Style::default().fg(theme.colors.error),
        ),
    ]
}

/// Lines for a message showing `block`, drawn `width` columns wide.
pub(super) fn diff_message_lines(
    block: &DiffBlock,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let (prompt, prompt_color, _) = message_style(MessageRole::System, theme);
    let mut lines = prefixed_lines(
        (prompt, prompt_color),
        &block.title,
        theme.colors.foreground,
        width,
        Measure::GRID,
    );
    let mut shown = 0;
    let mut hidden = 0;
    for file in &block.files {
        let view = DiffView::from_unified(&file.path, &file.diff);
        let mut header = vec![
            Span::styled(
                view.path().to_string(),
                Style::default()
                    .fg(theme.colors.foreground)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
        ];
        header.extend(diff_stat_spans(&view, theme));
        lines.push(Line::default());
        lines.push(Line::from(header));
        for line in view.styled_lines(theme, width) {
            if shown < MAX_DIFF_LINES {
                lines.push(line);
                shown += 1;
            } else {
                hidden += 1;
            }
        }
    }
    if hidden > 0 {
        lines.push(Line::from(Span::styled(
            format!("… {hidden} more lines"),
            Style::default()
                .fg(theme.colors.muted)
                .add_modifier(Modifier::ITALIC),
        )));
    }
    lines
}
