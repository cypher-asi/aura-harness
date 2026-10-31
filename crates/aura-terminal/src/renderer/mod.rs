//! Simple IRC-style renderer for the terminal UI.

mod input;
mod markdown;
mod overlays;
mod panels;
mod segments;
mod text;

#[cfg(test)]
mod tests;

use crate::{app::PanelAreas, App, Theme};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
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
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(area);
    let input_row = Rect {
        height: 1,
        ..main_chunks[2]
    };
    // Mirrors the panel border above so the input sits centered between two rules.
    let input_rule = Rect {
        y: main_chunks[2].y + 1,
        height: 1,
        ..main_chunks[2]
    };

    render_header(frame, inset_edges(main_chunks[0]), app, theme);

    render_content_panels(frame, main_chunks[1], app, theme);

    let swarm_offset = if app.swarm_panel_visible() {
        let swarm_percent: u32 = if app.record_panel_visible() { 20 } else { 25 };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "UI widths are always < u16::MAX"
        )]
        let offset = (u32::from(main_chunks[2].width) * swarm_percent / 100) as u16;
        offset
    } else {
        0
    };

    input::render_input(frame, input_row, app, theme, swarm_offset);
    frame.render_widget(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(theme.colors.muted)),
        input_rule,
    );

    overlays::render_overlays(frame, app, theme);
}

/// Render the content panels (Swarm, Chat, Record) based on visibility.
fn render_content_panels(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let swarm_visible = app.swarm_panel_visible();
    let record_visible = app.record_panel_visible();

    let areas = match (swarm_visible, record_visible) {
        (true, true) => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(20),
                    Constraint::Percentage(50),
                    Constraint::Percentage(30),
                ])
                .split(area);
            PanelAreas {
                swarm: Some(chunks[0]),
                agent_rows: None,
                chat: Some(chunks[1]),
                records: Some(chunks[2]),
            }
        }
        (true, false) => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
                .split(area);
            PanelAreas {
                swarm: Some(chunks[0]),
                agent_rows: None,
                chat: Some(chunks[1]),
                records: None,
            }
        }
        (false, true) => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
                .split(area);
            PanelAreas {
                swarm: None,
                agent_rows: None,
                chat: Some(chunks[0]),
                records: Some(chunks[1]),
            }
        }
        (false, false) => PanelAreas {
            swarm: None,
            agent_rows: None,
            chat: Some(area),
            records: None,
        },
    };
    app.set_panel_areas(areas);

    if let Some(swarm) = areas.swarm {
        panels::render_swarm_panel(frame, swarm, app, theme);
    }
    if let Some(chat) = areas.chat {
        panels::render_chat_panel(frame, chat, app, theme);
    }
    if let Some(records) = areas.records {
        panels::render_record_panel(frame, records, app, theme);
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

    let title = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("AURA", Style::default().fg(theme.colors.foreground)),
            Span::styled(" CLI", Style::default().fg(theme.colors.muted)),
        ]),
        Line::from(""),
    ];
    frame.render_widget(Paragraph::new(title), header_layout[0]);

    let right = vec![Line::from(""), Line::from(status_spans), Line::from("")];
    frame.render_widget(
        Paragraph::new(right).alignment(Alignment::Right),
        header_layout[1],
    );
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| text::display_width(&s.content)).sum()
}

/// `● local  http://127.0.0.1:8080`, or the user's name in place of `local`
/// when logged in. The dot reflects API connectivity.
fn header_status_spans<'a>(app: &'a App, theme: &Theme) -> Vec<Span<'a>> {
    let (icon, icon_color) = if app.api_active() {
        ("● ", theme.colors.primary)
    } else {
        ("○ ", theme.colors.error)
    };
    let who = app.user().map_or_else(
        || Span::styled("local", Style::default().fg(theme.colors.muted)),
        |user| Span::styled(user, Style::default().fg(theme.colors.primary)),
    );
    let mut spans = vec![Span::styled(icon, Style::default().fg(icon_color)), who];
    if let Some(url) = app.api_url() {
        spans.push(Span::styled(
            format!("  {url}"),
            Style::default().fg(theme.colors.muted),
        ));
    }
    spans
}

/// `qwen3:4b` — empty until the kernel reports a model.
pub(super) fn model_indicator_spans<'a>(app: &'a App, theme: &Theme) -> Vec<Span<'a>> {
    if app.current_model().is_empty() {
        return Vec::new();
    }
    let label = app
        .current_model_option()
        .map_or_else(|| app.current_model(), |m| m.label.as_str());
    vec![Span::styled(label, Style::default().fg(theme.colors.muted))]
}
