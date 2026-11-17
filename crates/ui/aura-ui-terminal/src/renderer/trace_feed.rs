//! TRACE tab: live feed of recorded steps and harness decisions.

use super::graph::status_glyph;
use super::panels::{cell_style, list_row, CellState, StatusIcon};
use crate::app::PanelFocus;
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Hint shown at the bottom of the tab when the panel has focus.
const TRACE_HINT: &str = "⏎ open in graph  ⇧←→ tabs";

/// Draw the feed into `inner`, the body below the tab bar.
pub(super) fn render_trace_feed(frame: &mut Frame, inner: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Sandbox;
    if app.trace_feed().entries().is_empty() {
        let lines: Vec<Line> = [
            "  No steps yet.",
            "  Model calls, tool calls and",
            "  harness decisions of this",
            "  session appear here.",
        ]
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
    let scroll = app.sync_trace_scroll(visible);
    app.set_trace_rows_area(rows_area, scroll);
    let width = inner.width as usize;
    let feed = app.trace_feed();
    let selected = feed.selected();
    let lines: Vec<Line> = feed
        .entries()
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .map(|(i, entry)| {
            let state = if is_focused && i == selected {
                CellState::Selected
            } else {
                CellState::Normal
            };
            let cell = cell_style(theme, state, Style::default().fg(theme.colors.foreground));
            let (glyph, color) = status_glyph(entry.node.status, theme);
            let name = if entry.node.detail.is_empty() {
                entry.node.label.clone()
            } else {
                format!("{}  {}", entry.node.label, entry.node.detail)
            };
            let seq = entry.node.seq.map(|seq| format!("#{seq}"));
            list_row(
                width,
                0,
                StatusIcon::new(glyph, color),
                &name,
                cell,
                seq.as_deref(),
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), rows_area);

    if show_hint {
        let hint = Rect {
            y: inner.y + inner.height - 1,
            height: 1,
            ..inner
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {TRACE_HINT}"),
                Style::default().fg(theme.colors.muted),
            )),
            hint,
        );
    }
}
