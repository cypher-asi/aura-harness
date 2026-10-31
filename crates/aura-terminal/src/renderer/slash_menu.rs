//! Slash-command menu, drawn over the bottom of the Chat panel so it sits
//! directly above the input line.

use super::text::display_width;
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Most commands listed at once; the list scrolls past this.
const MAX_ROWS: usize = 8;

/// Columns between the command and its description.
const DESCRIPTION_GAP: usize = 3;

/// Render the slash menu across the full width of `chat`, bottom-aligned
/// with it. Draws nothing while the menu is closed.
pub(super) fn render_slash_menu(frame: &mut Frame, chat: Rect, app: &App, theme: &Theme) {
    let matches = app.slash_matches();
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
    let cursor = app.slash_cursor();
    let scroll = (cursor + 1).saturating_sub(rows);

    let command_width = matches
        .iter()
        .map(|c| command_label_width(c.name, c.args))
        .max()
        .unwrap_or(0);

    let lines: Vec<Line<'_>> = matches
        .iter()
        .enumerate()
        .skip(scroll)
        .take(rows)
        .map(|(idx, command)| {
            let selected = idx == cursor;
            let (marker, name_color) = if selected {
                ("› ", theme.colors.primary)
            } else {
                ("  ", theme.colors.foreground)
            };
            let mut spans = vec![
                Span::styled(marker, Style::default().fg(theme.colors.primary)),
                Span::styled(
                    format!("/{}", command.name),
                    Style::default().fg(name_color),
                ),
            ];
            if !command.args.is_empty() {
                spans.push(Span::styled(
                    format!(" {}", command.args),
                    Style::default().fg(theme.colors.muted),
                ));
            }
            let pad = command_width - command_label_width(command.name, command.args);
            spans.push(Span::raw(" ".repeat(pad + DESCRIPTION_GAP)));
            spans.push(Span::styled(
                command.description,
                Style::default().fg(theme.colors.muted),
            ));
            Line::from(spans)
        })
        .collect();

    let block = Block::default()
        .title(" COMMANDS ")
        .title_bottom(
            Line::from(" ↑/↓ select  Tab complete  Enter run  Esc close ").right_aligned(),
        )
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.primary));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// Width of `/name args`.
fn command_label_width(name: &str, args: &str) -> usize {
    let args_width = if args.is_empty() {
        0
    } else {
        1 + display_width(args)
    };
    1 + display_width(name) + args_width
}
