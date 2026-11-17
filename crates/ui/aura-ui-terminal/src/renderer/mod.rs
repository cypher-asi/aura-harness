//! Simple IRC-style renderer for the terminal UI.

mod activity;
mod chat_lines;
mod command;
mod detail;
mod diff;
mod graph;
mod header;
mod hud;
mod input;
mod markdown;
mod mention_menu;
mod overlays;
mod panels;
mod plans;
mod processes;
mod sandbox;
mod segments;
mod slash_menu;
mod text;
mod trace_feed;

#[cfg(test)]
mod tests;

pub(crate) use chat_lines::{ChatLineCache, GroupState};
pub(crate) use panels::format_elapsed;

use crate::mode_command::{ApprovalLevel, PermissionMode};
use crate::{app::PanelAreas, App, Theme};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
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

/// Render the full application UI in IRC style.
pub fn render(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let area = frame.area();

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(area);
    let input_row = Rect {
        height: 1,
        ..main_chunks[2]
    };
    let status_row = Rect {
        y: input_row.y.saturating_add(1),
        height: main_chunks[2].height.saturating_sub(1),
        ..main_chunks[2]
    };

    render_header(frame, inset_edges(main_chunks[0]), app, theme);

    app.set_prose_rows(Vec::new());
    let chat = render_content_panels(frame, main_chunks[1], app, theme);
    hud::render_todo_strip(frame, chat, app, theme);
    detail::render_detail_view(frame, chat, app, theme);
    if let Some(selection) = app.selection() {
        selection.highlight(frame.buffer_mut());
    }
    let chat_left = chat.x.saturating_sub(input_row.x);

    input::render_input(frame, input_row, app, theme, chat_left);
    if status_row.height > 0 {
        hud::render_status_line(frame, inset_edges(status_row), app, theme);
    }

    hud::render_todo_panel(frame, chat, app, theme);
    slash_menu::render_slash_menu(frame, chat, app, theme);
    mention_menu::render_mention_menu(frame, chat, app, theme);
    overlays::render_folder_picker(frame, app, theme);
    overlays::render_overlays(frame, app, theme);
    app.drop_covered_prose(frame.buffer_mut());
}

/// Whether the right column (Record above Sandbox) has anything to show.
const fn right_column_visible(app: &App) -> bool {
    app.record_panel_visible() || app.sandbox_panel_visible()
}

/// Render the content panels (Swarm, Chat, Record, Sandbox) based on visibility,
/// returning where the Chat panel's body was drawn, below its header bar.
fn render_content_panels(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) -> Rect {
    app.clear_pane_chrome();
    let swarm_visible = app.swarm_panel_visible();
    let right_visible = right_column_visible(app);
    let splits = app.pane_splits();

    let room = area.width.saturating_sub(MIN_CHAT_WIDTH);
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
    let (swarm, chat, right) = match (swarm_visible, right_visible) {
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
                .constraints(side(splits.agents, 25))
                .split(area);
            (Some(chunks[0]), chunks[1], None)
        }
        (false, true) => {
            let [right, chat] = side(splits.right, 35);
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([chat, right])
                .split(area);
            (None, chunks[0], Some(chunks[1]))
        }
        (false, false) => (None, area, None),
    };
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

/// Columns the Chat panel keeps however far the dividers are dragged.
const MIN_CHAT_WIDTH: u16 = 30;

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
/// signed in: `● alice (zOS) · Claude · ChatGPT  http://...`. The dot
/// reflects API connectivity.
fn header_status_spans<'a>(app: &'a App, theme: &Theme) -> Vec<Span<'a>> {
    let (icon, icon_color) = if app.api_active() {
        ("● ", theme.colors.primary)
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
        spans.push(Span::styled(plan.as_str(), signed_in));
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

/// `ASK · qwen3:4b`: the approval level, then a `PLAN` / `READ-ONLY` /
/// `DEBUG` badge outside agent mode, then the model, or `Gate → <model>
/// (<route>)` when Gate picks it. The level and the model are left out
/// until the kernel reports them.
pub(super) fn model_indicator_spans<'a>(app: &'a App, theme: &Theme) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let separator = || Span::styled(" · ", Style::default().fg(theme.colors.muted));
    if let Some(level) = app.approval() {
        let (label, color) = match level {
            ApprovalLevel::Yolo => ("YOLO", theme.colors.error),
            ApprovalLevel::Ask => ("ASK", theme.colors.muted),
            ApprovalLevel::Strict => ("STRICT", theme.colors.primary),
        };
        spans.push(Span::styled(
            label,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    let badge = match app.mode() {
        PermissionMode::Agent => None,
        PermissionMode::Plan => Some("PLAN"),
        PermissionMode::ReadOnly => Some("READ-ONLY"),
        PermissionMode::Debug => Some("DEBUG"),
    };
    if let Some(badge) = badge {
        if !spans.is_empty() {
            spans.push(separator());
        }
        spans.push(Span::styled(
            badge,
            Style::default()
                .fg(theme.colors.warning)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if app.current_model().is_empty() {
        return spans;
    }
    if !spans.is_empty() {
        spans.push(separator());
    }
    let label = app
        .current_model_option()
        .map_or_else(|| app.current_model(), |m| m.label.as_str());
    let label = match app.model_route() {
        Some(route) => format!("{label} → {route}"),
        None => label.to_string(),
    };
    spans.push(Span::styled(label, Style::default().fg(theme.colors.muted)));
    spans
}
