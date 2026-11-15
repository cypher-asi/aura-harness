//! The panel below the Record panel: a SANDBOX | PROCESSES tab bar over the
//! file tree of the active agent's workspace or the Processes list.

use super::panels::{
    hover_style, render_scrollbar, tab_bar_split, tab_line, truncate_record_info, ScrollView,
};
use super::text::display_width;
use crate::app::sandbox::{SandboxEntryKind, SandboxRow};
use crate::app::{HoverTarget, PanelFocus, SidePanelTab};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// Hint shown at the bottom of the panel when it has focus.
const SANDBOX_HINT: &str = "↑↓ move  ⏎ open  ← up";

/// Render the panel: its tab bar, then the open tab.
pub(super) fn render_sandbox_panel(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Sandbox;
    let border_color = if is_focused {
        theme.colors.primary
    } else {
        theme.colors.muted
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let (tabs, body) = tab_bar_split(inner);
    app.set_side_tabs_area(tabs);
    let hovered = match app.hover() {
        Some(HoverTarget::SideTab(tab)) => Some(tab),
        _ => None,
    };
    let line = tab_line(
        SidePanelTab::ALL
            .map(|tab| (tab, tab.label(), app.side_tab_count(tab)))
            .into_iter(),
        app.side_tab(),
        hovered,
        is_focused,
        theme,
    );
    frame.render_widget(Paragraph::new(line), tabs);
    if body.height == 0 {
        return;
    }

    match app.side_tab() {
        SidePanelTab::Sandbox => render_tree(frame, area, body, app, theme),
        SidePanelTab::Processes => {
            super::processes::render_processes(frame, area, body, app, theme);
        }
    }
}

/// Draw the sandbox tree into `inner`, the body below the tab bar of the
/// panel bordered by `area`.
fn render_tree(frame: &mut Frame, area: Rect, inner: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Sandbox;
    if app.sandbox().rows().is_empty() {
        let message = if app.sandbox().root().is_some() {
            [
                "  Sandbox is empty.",
                "  Files the agent creates",
                "  will appear here.",
            ]
        } else {
            ["  No sandbox yet.", "", ""]
        };
        let lines: Vec<Line> = message
            .into_iter()
            .map(|text| Line::from(Span::styled(text, Style::default().fg(theme.colors.muted))))
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let show_hint = is_focused && inner.height >= 4;
    let rows_area = Rect {
        height: inner.height.saturating_sub(u16::from(show_hint)),
        ..inner
    };
    let visible = rows_area.height as usize;
    let scroll = app.sync_sandbox_scroll(visible);
    let hovered = match app.hover() {
        Some(HoverTarget::SandboxRow(index)) => Some(index),
        _ => None,
    };
    let tree = app.sandbox();
    let selected = tree.selected();

    let lines: Vec<Line> = tree
        .rows()
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .map(|(i, row)| {
            sandbox_line(
                row,
                is_focused && i == selected,
                hovered == Some(i),
                inner.width as usize,
                theme,
            )
        })
        .collect();
    let view = ScrollView {
        total: tree.rows().len(),
        visible,
        first: scroll,
    };
    app.set_sandbox_rows_area(rows_area, scroll);
    frame.render_widget(Paragraph::new(lines), rows_area);
    render_scrollbar(frame, area, rows_area, view, is_focused, theme);

    if show_hint {
        let hint_area = Rect {
            y: inner.y + inner.height - 1,
            height: 1,
            ..inner
        };
        let hint = truncate_record_info(SANDBOX_HINT, (inner.width as usize).saturating_sub(1));
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {hint}"),
                Style::default().fg(theme.colors.muted),
            ))),
            hint_area,
        );
    }
}

fn sandbox_line<'a>(
    row: &SandboxRow,
    highlighted: bool,
    hovered: bool,
    width: usize,
    theme: &Theme,
) -> Line<'a> {
    let prefix = if highlighted { ">" } else { " " };
    let indent = "  ".repeat(row.depth);
    let (marker, label, color) = match row.kind {
        SandboxEntryKind::Dir => (
            if row.expanded { "▾ " } else { "▸ " },
            format!("{}/", row.name),
            if row.name.starts_with('.') {
                theme.colors.muted
            } else {
                theme.colors.foreground
            },
        ),
        SandboxEntryKind::File => ("  ", row.name.clone(), theme.colors.muted),
        SandboxEntryKind::Symlink => ("  ", format!("{}@", row.name), theme.colors.muted),
        SandboxEntryKind::More(_) => ("  ", row.name.clone(), theme.colors.muted),
    };
    let style = if highlighted {
        Style::default()
            .fg(theme.colors.primary)
            .add_modifier(Modifier::BOLD)
    } else if hovered {
        hover_style(theme)
    } else {
        Style::default().fg(color)
    };
    let marker_style = if hovered && !highlighted {
        hover_style(theme)
    } else {
        Style::default().fg(theme.colors.muted)
    };
    let used = display_width(prefix) + indent.len() + display_width(marker);
    let label = truncate_record_info(&label, width.saturating_sub(used));
    Line::from(vec![
        Span::styled(prefix, style),
        Span::raw(indent),
        Span::styled(marker, marker_style),
        Span::styled(label, style),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dir_row(name: &str) -> SandboxRow {
        SandboxRow {
            path: PathBuf::from(name),
            name: name.to_string(),
            depth: 0,
            kind: SandboxEntryKind::Dir,
            expanded: false,
        }
    }

    fn label_color(row: &SandboxRow, theme: &Theme) -> Option<ratatui::style::Color> {
        sandbox_line(row, false, false, 40, theme).spans[3].style.fg
    }

    #[test]
    fn dot_folders_are_muted() {
        let theme = Theme::cyber();
        assert_eq!(
            label_color(&dir_row("src"), &theme),
            Some(theme.colors.foreground)
        );
        assert_eq!(
            label_color(&dir_row(".git"), &theme),
            Some(theme.colors.muted)
        );
    }
}
