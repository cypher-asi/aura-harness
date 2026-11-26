//! Slash-command menu, drawn over the bottom of the Chat panel so it sits
//! directly above the input line. After `/agent ` it lists permanent agents
//! to add to the project instead.

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

/// One row of the menu.
struct MenuRow<'a> {
    name: String,
    /// Argument hint after the name (empty when there is none)
    args: &'a str,
    description: &'a str,
}

impl MenuRow<'_> {
    /// Width of `name args`.
    fn label_width(&self) -> usize {
        let args_width = if self.args.is_empty() {
            0
        } else {
            1 + display_width(self.args)
        };
        display_width(&self.name) + args_width
    }
}

/// The menu's title, its key hint and its rows: the matching commands, or
/// the agents to add after `/agent `.
fn menu_rows(app: &App) -> (&'static str, &'static str, Vec<MenuRow<'_>>) {
    let commands = app.slash_matches();
    if commands.is_empty() {
        (
            " ADD AGENT TO PROJECT ",
            " ↑/↓ select  Tab complete  Enter add  Esc close ",
            app.agent_picks()
                .into_iter()
                .map(|definition| MenuRow {
                    name: definition.name.clone(),
                    args: "",
                    description: definition.description.lines().next().unwrap_or(""),
                })
                .collect(),
        )
    } else {
        (
            " COMMANDS ",
            " ↑/↓ select  Tab complete  Enter run  Esc close ",
            commands
                .into_iter()
                .map(|command| MenuRow {
                    name: format!("/{}", command.name),
                    args: command.args,
                    description: command.description,
                })
                .collect(),
        )
    }
}

/// One row of the menu, its description starting `label_width` columns
/// past the marker.
fn menu_line<'a>(row: &MenuRow<'a>, selected: bool, label_width: usize, theme: &Theme) -> Line<'a> {
    let (marker, name_color) = if selected {
        ("› ", theme.colors.primary)
    } else {
        ("  ", theme.colors.foreground)
    };
    let mut spans = vec![
        Span::styled(marker, Style::default().fg(theme.colors.primary)),
        Span::styled(row.name.clone(), Style::default().fg(name_color)),
    ];
    if !row.args.is_empty() {
        spans.push(Span::styled(
            format!(" {}", row.args),
            Style::default().fg(theme.colors.muted),
        ));
    }
    let pad = label_width - row.label_width();
    spans.push(Span::raw(" ".repeat(pad + DESCRIPTION_GAP)));
    spans.push(Span::styled(
        row.description,
        Style::default().fg(theme.colors.muted),
    ));
    Line::from(spans)
}

/// Render the slash menu across the full width of `chat`, bottom-aligned
/// with it. Draws nothing while the menu is closed.
pub(super) fn render_slash_menu(frame: &mut Frame, chat: Rect, app: &App, theme: &Theme) {
    let (title, hint, rows) = menu_rows(app);
    if rows.is_empty() || chat.height < 3 {
        return;
    }
    let height = u16::try_from(rows.len().min(MAX_ROWS))
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(chat.height);
    let area = Rect {
        y: chat.bottom() - height,
        height,
        ..chat
    };
    let visible = usize::from(height - 2);
    let cursor = app.slash_cursor();
    let scroll = (cursor + 1).saturating_sub(visible);
    let label_width = rows.iter().map(MenuRow::label_width).max().unwrap_or(0);

    let lines: Vec<Line<'_>> = rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .map(|(idx, row)| menu_line(row, idx == cursor, label_width, theme))
        .collect();

    let block = Block::default()
        .title(title)
        .title_bottom(Line::from(hint).right_aligned())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.muted));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
