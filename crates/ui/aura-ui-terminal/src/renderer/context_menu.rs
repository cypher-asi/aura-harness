//! The right-click menu of the projects list, drawn over the panels just
//! below and right of the cell clicked, and moved up or left to stay on
//! screen.

use super::text::display_width;
use crate::{app::AppState, App, Theme};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Columns of the highlight marker before each label.
const MARKER: &str = "› ";

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
    let width = u16::try_from(widest + display_width(MARKER) + 3)
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
            let (marker, color) = if index == selected {
                (MARKER, theme.colors.primary)
            } else {
                ("  ", theme.colors.foreground)
            };
            Line::from(vec![
                Span::styled(marker, Style::default().fg(theme.colors.primary)),
                Span::styled(*label, Style::default().fg(color)),
            ])
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.muted));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
    app.set_context_menu_area(area);
}
