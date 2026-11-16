//! `@` file picker, drawn over the bottom of the Chat panel like the
//! slash menu so it sits directly above the input line.

use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Most matches listed at once; the list scrolls past this.
const MAX_ROWS: usize = 8;

/// Render the `@` picker across the full width of `chat`, bottom-aligned
/// with it. Draws nothing while the picker is closed.
pub(super) fn render_mention_menu(frame: &mut Frame, chat: Rect, app: &App, theme: &Theme) {
    let matches = app.mention_matches();
    if matches.is_empty() || chat.height < 3 {
        return;
    }
    let height = u16::try_from(matches.len().min(MAX_ROWS))
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(chat.height);
    let area = Rect {
        y: chat.bottom() - height,
        height,
        ..chat
    };
    let rows = usize::from(height - 2);
    let cursor = app.mention_cursor();
    let scroll = (cursor + 1).saturating_sub(rows);

    let lines: Vec<Line<'_>> = matches
        .iter()
        .enumerate()
        .skip(scroll)
        .take(rows)
        .map(|(idx, candidate)| {
            let (marker, color) = if idx == cursor {
                ("› ", theme.colors.primary)
            } else if candidate.is_dir {
                ("  ", theme.colors.secondary)
            } else {
                ("  ", theme.colors.foreground)
            };
            Line::from(vec![
                Span::styled(marker, Style::default().fg(theme.colors.primary)),
                Span::styled(format!("@{}", candidate.path), Style::default().fg(color)),
            ])
        })
        .collect();

    let block = Block::default()
        .title(" FILES ")
        .title_bottom(
            Line::from(" ↑/↓ select  Tab complete  Enter insert  Esc close ").right_aligned(),
        )
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.primary));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
