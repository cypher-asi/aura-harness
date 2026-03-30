//! Simple IRC-style renderer for the terminal UI.

mod input;
mod markdown;
mod overlays;
mod panels;
mod text;

#[cfg(test)]
mod tests;

use crate::{App, Theme};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Render the full application UI in IRC style.
pub fn render(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let area = frame.area();

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, main_chunks[0], app, theme);

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

    input::render_input(frame, main_chunks[2], app, theme, swarm_offset);

    overlays::render_overlays(frame, app, theme);
}

/// Render the content panels (Swarm, Chat, Record) based on visibility.
fn render_content_panels(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let swarm_visible = app.swarm_panel_visible();
    let record_visible = app.record_panel_visible();

    match (swarm_visible, record_visible) {
        (true, true) => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(20),
                    Constraint::Percentage(50),
                    Constraint::Percentage(30),
                ])
                .split(area);
            panels::render_swarm_panel(frame, chunks[0], app, theme);
            panels::render_chat_panel(frame, chunks[1], app, theme);
            panels::render_record_panel(frame, chunks[2], app, theme);
        }
        (true, false) => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
                .split(area);
            panels::render_swarm_panel(frame, chunks[0], app, theme);
            panels::render_chat_panel(frame, chunks[1], app, theme);
        }
        (false, true) => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
                .split(area);
            panels::render_chat_panel(frame, chunks[0], app, theme);
            panels::render_record_panel(frame, chunks[1], app, theme);
        }
        (false, false) => {
            panels::render_chat_panel(frame, area, app, theme);
        }
    }
}

/// Render the header bar.
fn render_header(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let header_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(30)])
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

    if let Some(url) = app.api_url() {
        let (icon, color) = if app.api_active() {
            (" ● ", theme.colors.primary)
        } else {
            (" ○ ", theme.colors.error)
        };

        let api_spans = vec![
            Span::styled(icon, Style::default().fg(color)),
            Span::styled(url, Style::default().fg(theme.colors.muted)),
        ];

        let api_status = vec![Line::from(""), Line::from(api_spans), Line::from("")];
        frame.render_widget(
            Paragraph::new(api_status).alignment(Alignment::Right),
            header_layout[1],
        );
    }
}
