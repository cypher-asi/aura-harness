//! The right-click menu of the projects list, drawn over the panels just
//! below and right of the cell clicked, and moved up or left to stay on
//! screen. Its items are drawn like list rows: the highlighted one, under
//! the pointer or the keyboard cursor, gets the hover background across the
//! whole row.
//!
//! An item with a submenu ends in `›`. Its submenu, the workspaces to move
//! a project to, is a second box with its first entry on the item's row,
//! right of the menu, or left of it when there is no room on the right,
//! and moved up to stay on screen.

use super::hud::truncate;
use super::text::display_width;
use crate::app::{AppState, SubmenuEntry, WorkspaceTarget};
use crate::components::{cell_style, CellState};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Blank space on each side of a label.
const GUTTER: &str = " ";
/// What ends the row of an item that opens a submenu.
const SUBMENU_MARK: &str = "›";
/// Least space between a label and [`SUBMENU_MARK`].
const MARK_GAP: usize = 2;
/// Widest a workspace's title gets in the submenu, in cells.
const MAX_TITLE: usize = 28;

/// Render the context menu, if one is open, and record where it went.
pub(super) fn render_context_menu(frame: &mut Frame, app: &mut App, theme: &Theme) {
    if !matches!(app.state(), AppState::Idle | AppState::Processing) {
        return;
    }
    let Some(menu) = app.context_menu() else {
        return;
    };
    let rows: Vec<(&str, bool)> = menu
        .items
        .iter()
        .map(|&item| (app.menu_label(menu, item), item.has_submenu()))
        .collect();
    let (anchor, selected) = (menu.anchor, menu.selected);
    // A submenu is only open beside the highlighted item.
    let parent = menu.submenu.is_some().then_some(menu.selected);

    let screen = frame.area();
    let widest = rows
        .iter()
        .map(|&(label, opens)| display_width(label) + if opens { MARK_GAP + 1 } else { 0 })
        .max()
        .unwrap_or(0);
    let inner_width = widest + 2 * GUTTER.len();
    let width = u16::try_from(inner_width + 2)
        .unwrap_or(u16::MAX)
        .min(screen.width);
    let height = u16::try_from(rows.len() + 2)
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

    let lines: Vec<Line<'_>> = rows
        .iter()
        .enumerate()
        .map(|(index, &(label, opens))| {
            let text = if opens {
                let pad = inner_width.saturating_sub(display_width(label) + 2 * GUTTER.len() + 1);
                format!("{GUTTER}{label}{}{SUBMENU_MARK}{GUTTER}", " ".repeat(pad))
            } else {
                let pad = inner_width.saturating_sub(display_width(label) + GUTTER.len());
                format!("{GUTTER}{label}{}", " ".repeat(pad))
            };
            row_line(theme, text, index == selected)
        })
        .collect();
    draw_box(frame, theme, area, lines);
    app.set_context_menu_area(area);

    if let Some(parent) = parent {
        let row = area
            .y
            .saturating_add(1)
            .saturating_add(u16::try_from(parent).unwrap_or(u16::MAX));
        render_submenu(frame, app, theme, area, row);
    }
}

/// Draw the open submenu beside `menu`, its first entry on `row`, and
/// record where it went.
fn render_submenu(frame: &mut Frame, app: &mut App, theme: &Theme, menu: Rect, row: u16) {
    let Some(submenu) = app.context_menu().and_then(|menu| menu.submenu.as_ref()) else {
        return;
    };
    let labels: Vec<String> = submenu
        .entries
        .iter()
        .map(|entry| entry_label(app.workspace_labels(), entry))
        .collect();
    let selected = submenu.selected;

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
    let x = if menu.right().saturating_add(width) <= screen.right() {
        menu.right()
    } else if menu.x.saturating_sub(screen.x) >= width {
        menu.x - width
    } else {
        // Room on neither side: as far right as it fits, over the menu.
        screen.right().saturating_sub(width)
    };
    let y = row
        .saturating_sub(1)
        .min(screen.bottom().saturating_sub(height))
        .max(screen.y);
    let area = Rect {
        x,
        y,
        width,
        height,
    };

    let lines: Vec<Line<'_>> = labels
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            let pad = inner_width.saturating_sub(display_width(&label) + GUTTER.len());
            row_line(
                theme,
                format!("{GUTTER}{label}{}", " ".repeat(pad)),
                index == selected,
            )
        })
        .collect();
    draw_box(frame, theme, area, lines);
    app.set_context_submenu_area(area);
}

/// What a submenu entry reads: `Workspace 2  NOTES / CC` or `New
/// workspace`. Workspaces are numbered from 1, as the tab bar shows them.
fn entry_label(titles: &[String], entry: &SubmenuEntry) -> String {
    let SubmenuEntry::Workspace(target) = *entry;
    match target {
        WorkspaceTarget::New => "New workspace".to_string(),
        WorkspaceTarget::Existing(index) => {
            let number = index + 1;
            match titles.get(index).map(|title| title.trim()) {
                Some(title) if !title.is_empty() => {
                    let title = truncate(title, MAX_TITLE);
                    format!("Workspace {number}  {title}")
                }
                _ => format!("Workspace {number}"),
            }
        }
    }
}

/// A row of a menu, lit across when it is the highlighted one.
fn row_line(theme: &Theme, text: String, selected: bool) -> Line<'static> {
    let state = CellState::of(false, selected);
    let cell = cell_style(theme, state, Style::default().fg(theme.colors.foreground));
    let style = cell.bg.map_or(cell.text, |bg| cell.text.bg(bg));
    Line::styled(text, style)
}

fn draw_box(frame: &mut Frame, theme: &Theme, area: Rect, lines: Vec<Line<'_>>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.muted));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
