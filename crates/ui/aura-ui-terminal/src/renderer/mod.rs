//! Simple IRC-style renderer for the terminal UI.

mod activity;
mod chat_lines;
mod command;
mod detail;
mod diff;
mod flow;
mod graph;
mod header;
mod hud;
mod images;
mod input;
mod markdown;
mod mention_menu;
mod overlays;
mod panels;
mod plans;
mod processes;
mod sandbox;
mod segments;
mod side_list;
mod slash_menu;
mod text;
mod trace_feed;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

pub(crate) use chat_lines::{ChatLineCache, GroupState};
pub(crate) use panels::format_elapsed;

use crate::mode_command::ApprovalLevel;
use crate::{app::PanelAreas, App, Theme};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Columns that text outside the panels keeps from the screen edges.
/// Border glyphs are drawn through the middle of their cell, so text in the
/// outermost column would overhang the panel borders.
pub(super) const EDGE_INSET: u16 = 1;

fn inset_edges(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(EDGE_INSET),
        width: area.width.saturating_sub(EDGE_INSET * 2),
        ..area
    }
}

/// Render the full application UI: the header row, then the panels, with
/// the input box and the footer under the Chat panel. The side columns end
/// level with the input box.
pub fn render(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3)])
        .split(area);
    let content = inset_edges(rows[1]);

    // The input grows upward as it wraps inside the Chat column, so its
    // height comes from where the columns fall.
    let (swarm, column, right) = columns(content, app);
    let footer_rows = hud::footer_rows(content.height);
    let max_input_rows = content
        .height
        .saturating_sub(footer_rows + input::BOX_ROWS + 3)
        .max(1);
    let input_rows = input::input_rows(column, app, theme, max_input_rows);
    let chat_column = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(input_rows + input::BOX_ROWS),
            Constraint::Length(footer_rows),
        ])
        .split(column);
    let above_footer = |side: Rect| Rect {
        height: side.height.saturating_sub(footer_rows),
        ..side
    };
    let (swarm, right) = (swarm.map(above_footer), right.map(above_footer));

    render_header(frame, inset_edges(rows[0]), app, theme);

    app.set_prose_rows(Vec::new());
    app.clear_chat_scroll();
    let chat = render_content_panels(frame, (swarm, chat_column[0], right), app, theme);
    images::render_pending_images(frame, chat, app, theme);
    detail::render_detail_view(frame, chat, app, theme);
    if let Some(selection) = app.selection() {
        selection.highlight(frame.buffer_mut());
    }
    input::render_input(frame, chat_column[1], app, theme);
    hud::render_footer(frame, inset_edges(chat_column[2]), app, theme);

    hud::render_todo_panel(frame, chat, app, theme);
    slash_menu::render_slash_menu(frame, chat, app, theme);
    mention_menu::render_mention_menu(frame, chat, app, theme);
    overlays::render_folder_picker(frame, app, theme);
    overlays::render_overlays(frame, app, theme);
    app.drop_covered_prose(frame.buffer_mut());
    app.drop_covered_chat(frame.buffer_mut());
}

/// Whether the right column (Record above Sandbox) has anything to show.
const fn right_column_visible(app: &App) -> bool {
    app.record_panel_visible() || app.sandbox_panel_visible()
}

/// Render the content panels (Swarm, Chat, Record, Sandbox) in the columns
/// [`columns`] gave them, returning where the Chat panel's body was drawn,
/// below its header bar.
fn render_content_panels(
    frame: &mut Frame,
    (swarm, chat, right): (Option<Rect>, Rect, Option<Rect>),
    app: &mut App,
    theme: &Theme,
) -> Rect {
    app.clear_pane_chrome();
    let (records, sandbox) = split_right_column(right, app);
    let areas = PanelAreas {
        swarm,
        chat: Some(chat),
        records,
        sandbox,
        ..PanelAreas::default()
    };
    app.set_panel_areas(areas);

    if let Some(swarm) = areas.swarm {
        panels::render_agents_panel(frame, swarm, app, theme);
    }
    if let Some(chat) = areas.chat {
        panels::render_chat_panel(frame, chat, app, theme);
    }
    if let Some(records) = areas.records {
        panels::render_record_panel(frame, records, app, theme);
    }
    if let Some(sandbox) = areas.sandbox {
        sandbox::render_sandbox_panel(frame, sandbox, app, theme);
    }
    header::split(chat, app.host_chrome()).1
}

/// Split `area` into the Agents, Chat and right columns. Only `area`'s
/// width decides where the columns go.
fn columns(area: Rect, app: &App) -> (Option<Rect>, Rect, Option<Rect>) {
    let swarm_visible = app.swarm_panel_visible();
    let right_visible = right_column_visible(app);
    let splits = app.pane_splits();

    let gutters = GUTTER * (u16::from(swarm_visible) + u16::from(right_visible));
    let room = area.width.saturating_sub(MIN_CHAT_WIDTH + gutters);
    let side = |split: Option<f32>, percent: u16| match split {
        Some(split) => [
            Constraint::Length(dragged(area.width, split).min(room)),
            Constraint::Fill(1),
        ],
        None => [
            Constraint::Percentage(percent),
            Constraint::Percentage(100 - percent),
        ],
    };
    match (swarm_visible, right_visible) {
        (true, true) => {
            let default = area.width * 24 / 100;
            let left = splits
                .agents
                .map_or(default, |split| dragged(area.width, split).min(room / 2));
            let right = splits.right.map_or(default, |split| {
                dragged(area.width, split).min(room.saturating_sub(left))
            });
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .spacing(GUTTER)
                .constraints([
                    Constraint::Length(left),
                    Constraint::Fill(1),
                    Constraint::Length(right),
                ])
                .split(area);
            (Some(chunks[0]), chunks[1], Some(chunks[2]))
        }
        (true, false) => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .spacing(GUTTER)
                .constraints(side(splits.agents, 25))
                .split(area);
            (Some(chunks[0]), chunks[1], None)
        }
        (false, true) => {
            let [right, chat] = side(splits.right, 35);
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .spacing(GUTTER)
                .constraints([chat, right])
                .split(area);
            (None, chunks[0], Some(chunks[1]))
        }
        (false, false) => (None, area, None),
    }
}

/// Columns the Chat panel keeps however far the dividers are dragged.
const MIN_CHAT_WIDTH: u16 = 30;

/// Blank columns between panels side by side. Cells are about twice as
/// tall as they are wide, so one column matches the gap between panels
/// stacked on rows that touch.
const GUTTER: u16 = 1;

/// Rows each panel of the right column keeps when they share it.
const MIN_STACKED_HEIGHT: u16 = 6;

/// The cells a divider dragged to `split` of `total` leaves before it.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn dragged(total: u16, split: f32) -> u16 {
    (f32::from(total) * split.clamp(0.0, 1.0)).round() as u16
}

/// Record on top and Sandbox below, halves unless the divider between
/// them was moved; either one takes the whole column when the other is
/// hidden.
fn split_right_column(right: Option<Rect>, app: &App) -> (Option<Rect>, Option<Rect>) {
    let Some(area) = right else {
        return (None, None);
    };
    match (app.record_panel_visible(), app.sandbox_panel_visible()) {
        (true, true) => {
            let constraints = app.pane_splits().records.map_or(
                [Constraint::Percentage(50), Constraint::Percentage(50)],
                |split| {
                    let keep = MIN_STACKED_HEIGHT.min(area.height / 2);
                    let top = dragged(area.height, split).clamp(keep, area.height - keep);
                    [Constraint::Length(top), Constraint::Fill(1)]
                },
            );
            let halves = Layout::default()
                .direction(Direction::Vertical)
                .constraints(constraints)
                .split(area);
            (Some(halves[0]), Some(halves[1]))
        }
        (true, false) => (Some(area), None),
        (false, true) => (None, Some(area)),
        (false, false) => (None, None),
    }
}

/// Render the header bar.
fn render_header(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let status_spans = header_status_spans(app, theme);
    let status_width = spans_width(&status_spans);
    let right_width = u16::try_from(status_width.max(30) + 1).unwrap_or(u16::MAX);

    let header_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(right_width)])
        .split(area);

    let title = Line::from(vec![
        Span::styled("AURA", Style::default().fg(theme.colors.foreground)),
        Span::styled(" CLI", Style::default().fg(theme.colors.muted)),
    ]);
    frame.render_widget(Paragraph::new(title), header_layout[0]);

    let right = Line::from(status_spans);
    frame.render_widget(
        Paragraph::new(right).alignment(Alignment::Right),
        header_layout[1],
    );
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| text::display_width(&s.content)).sum()
}

/// `● local  http://127.0.0.1:8080` when signed out, otherwise where you are
/// signed in: `● alice (zOS) · CLAUDE MAX 20X · CHATGPT  http://...`. The dot
/// reflects API connectivity.
fn header_status_spans<'a>(app: &'a App, theme: &Theme) -> Vec<Span<'a>> {
    let (icon, icon_color) = if app.api_active() {
        ("● ", theme.colors.success)
    } else {
        ("○ ", theme.colors.error)
    };
    let muted = Style::default().fg(theme.colors.muted);
    let signed_in = Style::default().fg(theme.colors.primary);
    let mut spans = vec![Span::styled(icon, Style::default().fg(icon_color))];
    if let Some(user) = app.user() {
        spans.push(Span::styled(user, signed_in));
        spans.push(Span::styled(" (zOS)", muted));
    }
    for plan in app.plans() {
        if spans.len() > 1 {
            spans.push(Span::styled(" · ", muted));
        }
        spans.push(Span::styled(plan.to_uppercase(), signed_in));
    }
    if spans.len() == 1 {
        spans.push(Span::styled("local", muted));
    }
    if let Some(url) = app.api_url() {
        spans.push(Span::styled(
            format!("  {url}"),
            Style::default().fg(theme.colors.muted),
        ));
    }
    spans
}

/// `ASK · qwen3:4b`: the approval level, then the model, or `GATE` when
/// Gate picks it. Each is left out until the kernel reports it.
pub(super) fn model_indicator_spans<'a>(app: &'a App, theme: &Theme) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let separator = || Span::styled(" · ", Style::default().fg(theme.colors.faint));
    if let Some(level) = app.approval() {
        let (label, color) = match level {
            ApprovalLevel::Yolo => ("YOLO", theme.colors.foreground),
            ApprovalLevel::Ask => ("ASK", theme.colors.muted),
            ApprovalLevel::Strict => ("STRICT", theme.colors.foreground),
        };
        spans.push(Span::styled(label, Style::default().fg(color)));
    }
    if app.current_model().is_empty() {
        return spans;
    }
    if !spans.is_empty() {
        spans.push(separator());
    }
    if app.current_model() == "gate" {
        spans.push(Span::styled(
            "GATE",
            Style::default().fg(theme.colors.foreground),
        ));
        return spans;
    }
    let label = app
        .current_model_option()
        .map_or_else(|| app.current_model(), |m| m.label.as_str());
    spans.push(Span::styled(label, Style::default().fg(theme.colors.muted)));
    spans
}
