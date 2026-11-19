//! The right-click menu of the projects list, drawn over the panels just
//! below and right of the cell clicked, and moved up or left to stay on
//! screen. Its items are drawn like list rows: the highlighted one, under
//! the pointer or the keyboard cursor, gets the hover background across the
//! whole row.

use super::panels::{cell_style, CellState};
use super::text::display_width;
use crate::{app::AppState, App, Theme};
use ratatui::{
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Blank space on each side of a label.
const GUTTER: &str = " ";

/// Render the context menu, if one is open, and record where it went.
pub(super) fn render_context_menu(frame: &mut Frame, app: &mut App, theme: &Theme) {
    if !matches!(app.state(), AppState::Idle | AppState::Processing) {
        return;
    }
    let Some(menu) = app.context_menu() else {
        return;
    };
    let labels: Vec<&str> = menu
        .items
        .iter()
        .map(|&item| app.menu_label(menu, item))
        .collect();
    let (anchor, selected) = (menu.anchor, menu.selected);

    let screen = frame.area();
    let widest = labels
        .iter()
        .map(|label| display_width(label))
        .max()
        .unwrap_or(0);
    let inner_width = widest + 2 * GUTTER.len();
    let width = u16::try_from(inner_width + 2)
        .unwrap_or(u16::MAX)
        .min(screen.width);
    let height = u16::try_from(labels.len() + 2)
        .unwrap_or(u16::MAX)
        .min(screen.height);
    let x = anchor.x.min(screen.right().saturating_sub(width));
    let below = anchor.y.saturating_add(1);
    let y = if below.saturating_add(height) <= screen.bottom() {
        below
    } else {
        anchor.y.saturating_sub(height).max(screen.y)
    };
    let area = Rect {
        x,
        y,
        width,
        height,
    };

    let lines: Vec<Line<'_>> = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let state = CellState::of(false, index == selected);
            let cell = cell_style(theme, state, Style::default().fg(theme.colors.foreground));
            let style = cell.bg.map_or(cell.text, |bg| cell.text.bg(bg));
            let pad = inner_width.saturating_sub(display_width(label) + GUTTER.len());
            Line::styled(format!("{GUTTER}{label}{}", " ".repeat(pad)), style)
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.muted));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
    app.set_context_menu_area(area);
}
