//! TRACE tab: live feed of recorded steps and harness decisions.

use super::graph::status_glyph;
use super::panels::{
    cell_style, list_row_with_detail, render_scrollbar, CellState, ScrollView, StatusIcon,
};
use super::side_list::{
    name_style, render_empty, render_hidden, render_hint, split_hint, Tone, TRACE_HINT,
};
use crate::app::{HoverTarget, PanelFocus};
use crate::events::TraceNodeStatus;
use crate::{App, Theme};
use ratatui::{layout::Rect, text::Line, widgets::Paragraph, Frame};

/// Draw the feed into `inner`, the body below the tab bar of the panel
/// bordered by `area`.
pub(super) fn render_trace_feed(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let is_focused = app.focus() == PanelFocus::Sandbox;
    let feed = app.trace_feed();
    if feed.is_empty() && feed.hidden() > 0 {
        render_hidden(frame, inner, ("steps", feed.hidden()), theme);
        return;
    }
    if feed.is_empty() {
        let message = [
            "No steps yet.",
            "Model calls, tool calls and",
            "harness decisions of this",
            "session appear here.",
        ];
        render_empty(frame, inner, &message, theme);
        return;
    }

    let (rows_area, hint) = split_hint(inner, is_focused);
    let visible = rows_area.height as usize;
    let scroll = app.sync_trace_scroll(visible);
    app.set_trace_rows_area(rows_area, scroll);
    let hovered = match app.hover() {
        Some(HoverTarget::TraceRow(index)) => Some(index),
        _ => None,
    };
    let width = inner.width as usize;
    let feed = app.trace_feed();
    let selected = feed.selected();
    let lines: Vec<Line> = feed
        .entries()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .map(|(i, entry)| {
            let state = CellState::of(is_focused && i == selected, hovered == Some(i));
            let tone = Tone::active_if(entry.node.status == TraceNodeStatus::Pending);
            let cell = cell_style(theme, state, name_style(tone, theme));
            let (glyph, color) = status_glyph(entry.node.status, theme);
            let seq = entry.node.seq.map(|seq| format!("#{seq}"));
            list_row_with_detail(
                width,
                0,
                StatusIcon::new(glyph, color),
                (&entry.node.label, Some(&entry.node.detail)),
                cell,
                seq.as_deref(),
            )
        })
        .collect();
    let view = ScrollView {
        total: feed.len(),
        visible,
        first: scroll,
    };
    frame.render_widget(Paragraph::new(lines), rows_area);
    render_scrollbar(frame, area, rows_area, view, is_focused, theme);
    if let Some(hint) = hint {
        render_hint(frame, hint, TRACE_HINT, theme);
    }
}
