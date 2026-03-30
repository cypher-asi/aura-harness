//! Overlay rendering: approval modal, help, record detail, notifications.

use super::markdown::parse_markdown_line;
use super::text::{centered_rect, display_width, wrap_words};
use crate::{
    app::{AppState, NotificationType},
    App, Theme,
};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

/// Render overlay elements (modals, help).
pub(super) fn render_overlays(frame: &mut Frame, app: &App, theme: &Theme) {
    if app.showing_record_detail() {
        render_record_detail(frame, app, theme);
    }

    if let Some(approval) = app.pending_approval() {
        render_approval_modal(frame, approval, theme);
    }

    if app.state() == AppState::ShowingHelp {
        render_help_overlay(frame, theme);
    }

    if let Some((msg, notification_type)) = app.notification() {
        render_notification(frame, msg, *notification_type, theme);
    }
}

/// Render the approval modal.
fn render_approval_modal(frame: &mut Frame, approval: &crate::app::PendingApproval, theme: &Theme) {
    let area = frame.area();
    let modal_width = 50.min(area.width.saturating_sub(4));
    let modal_height = 6;

    let modal_area = centered_rect(modal_width, modal_height, area);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" Approval Required ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.warning));

    let content = vec![
        Line::from(vec![Span::styled(
            format!("{} wants to: ", approval.tool),
            Style::default().fg(theme.colors.foreground),
        )]),
        Line::from(Span::styled(
            &approval.description,
            Style::default().fg(theme.colors.muted),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("[Y] Yes", Style::default().fg(theme.colors.success)),
            Span::raw("  "),
            Span::styled("[N] No", Style::default().fg(theme.colors.error)),
        ]),
    ];

    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, modal_area);
}

/// Render the help overlay.
fn render_help_overlay(frame: &mut Frame, theme: &Theme) {
    let area = frame.area();
    let modal_width = 50.min(area.width.saturating_sub(4));
    let modal_height = 22;

    let modal_area = centered_rect(modal_width, modal_height, area);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.primary));

    let help_text = build_help_lines(theme);

    let paragraph = Paragraph::new(help_text)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, modal_area);
}

fn build_help_lines(theme: &Theme) -> Vec<Line<'static>> {
    let fg = theme.colors.foreground;
    let items = [
        "/help      Show this help",
        "/new       New session (reset context)",
        "/clear     Clear messages",
        "/swarm     Toggle Swarm panel",
        "/record    Toggle Record panel",
        "/login     Login to zOS",
        "/logout    Clear credentials",
        "/whoami     Show auth status",
        "/quit      Exit",
        "",
        "Enter      Send message",
        "Tab        Switch panels",
        "↑/↓        Navigate / History",
        "PgUp/PgDn  Scroll chat history",
        "Shift+Mouse Select text to copy",
        "Ctrl+C     Cancel/Exit",
        "",
        "Press any key to close",
    ];

    items
        .into_iter()
        .map(|s| {
            if s.is_empty() {
                Line::from("")
            } else if s == "Press any key to close" {
                Line::from(Span::styled(s, Style::default().fg(theme.colors.muted)))
            } else {
                Line::from(Span::styled(s, Style::default().fg(fg)))
            }
        })
        .collect()
}

/// Render the record detail overlay.
fn render_record_detail(frame: &mut Frame, app: &App, theme: &Theme) {
    use crate::events::RecordStatus;

    let Some(record) = app.selected_record_data() else {
        return;
    };

    let area = frame.area();
    let modal_width = 70.min(area.width.saturating_sub(4));
    let has_error = !record.error_details.is_empty();
    let base_height = 24u16;
    #[allow(clippy::cast_possible_truncation)]
    let error_lines = if has_error {
        3 + (record.error_details.len() / 60) as u16
    } else {
        0
    };
    let modal_height = (base_height + error_lines).min(area.height.saturating_sub(4));

    let modal_area = centered_rect(modal_width, modal_height, area);
    frame.render_widget(Clear, modal_area);

    let (status_icon, status_color) = match record.status {
        RecordStatus::Ok => (" ✓ ", theme.colors.success),
        RecordStatus::Error => (" ✗ ", theme.colors.error),
        RecordStatus::Pending => (" ◌ ", theme.colors.pending),
        RecordStatus::None => (" · ", theme.colors.muted),
    };

    let block = Block::default()
        .title(format!(" Record #{} {} ", record.seq, status_icon))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(status_color));

    let mut content = build_record_header(record, theme);
    build_transaction_section(&mut content, record, theme);
    build_processing_section(&mut content, record, theme);

    if has_error {
        build_error_section(&mut content, record, modal_width, theme);
    }

    build_message_section(&mut content, record, theme);

    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "Press Esc or Enter to close",
        Style::default().fg(theme.colors.muted),
    )));

    let paragraph = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, modal_area);
}

fn build_record_header<'a>(
    record: &'a crate::events::RecordSummary,
    theme: &Theme,
) -> Vec<Line<'a>> {
    vec![
        Line::from(vec![
            Span::styled("Sequence:    ", Style::default().fg(theme.colors.muted)),
            Span::styled(
                format!("{}", record.seq),
                Style::default().fg(theme.colors.foreground),
            ),
        ]),
        Line::from(vec![
            Span::styled("Context Hash:", Style::default().fg(theme.colors.muted)),
            Span::styled(
                format!(" {}", &record.full_hash),
                Style::default().fg(theme.colors.secondary),
            ),
        ]),
    ]
}

fn build_transaction_section<'a>(
    content: &mut Vec<Line<'a>>,
    record: &'a crate::events::RecordSummary,
    theme: &Theme,
) {
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "── Transaction ──",
        Style::default().fg(theme.colors.muted),
    )));
    content.push(Line::from(vec![
        Span::styled("tx_id:       ", Style::default().fg(theme.colors.muted)),
        Span::styled(&record.tx_id, Style::default().fg(theme.colors.secondary)),
    ]));
    content.push(Line::from(vec![
        Span::styled("agent_id:    ", Style::default().fg(theme.colors.muted)),
        Span::styled(
            &record.agent_id,
            Style::default().fg(theme.colors.secondary),
        ),
    ]));
    content.push(Line::from(vec![
        Span::styled("ts_ms:       ", Style::default().fg(theme.colors.muted)),
        Span::styled(
            format!("{}", record.ts_ms),
            Style::default().fg(theme.colors.foreground),
        ),
        Span::styled(
            format!(" ({})", record.timestamp),
            Style::default().fg(theme.colors.muted),
        ),
    ]));
    content.push(Line::from(vec![
        Span::styled("kind:        ", Style::default().fg(theme.colors.muted)),
        Span::styled(
            &record.tx_kind,
            Style::default().fg(theme.colors.foreground),
        ),
    ]));

    if !record.info.is_empty() {
        content.push(Line::from(vec![
            Span::styled("info:        ", Style::default().fg(theme.colors.muted)),
            Span::styled(&record.info, Style::default().fg(theme.colors.primary)),
        ]));
    }
}

fn build_processing_section<'a>(
    content: &mut Vec<Line<'a>>,
    record: &'a crate::events::RecordSummary,
    theme: &Theme,
) {
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "── Processing ──",
        Style::default().fg(theme.colors.muted),
    )));
    content.push(Line::from(vec![
        Span::styled("Sender:      ", Style::default().fg(theme.colors.muted)),
        Span::styled(&record.sender, Style::default().fg(theme.colors.foreground)),
    ]));
    content.push(Line::from(vec![
        Span::styled("Actions:     ", Style::default().fg(theme.colors.muted)),
        Span::styled(
            format!("{}", record.action_count),
            Style::default().fg(theme.colors.secondary),
        ),
    ]));
    content.push(Line::from(vec![
        Span::styled("Effects:     ", Style::default().fg(theme.colors.muted)),
        Span::styled(
            &record.effect_status,
            Style::default().fg(theme.colors.foreground),
        ),
    ]));
}

fn build_error_section<'a>(
    content: &mut Vec<Line<'a>>,
    record: &'a crate::events::RecordSummary,
    modal_width: u16,
    theme: &Theme,
) {
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "Error Details:",
        Style::default().fg(theme.colors.error),
    )));
    for line in record.error_details.lines() {
        if line.is_empty() {
            content.push(Line::from(""));
        } else {
            let max_width = (modal_width as usize).saturating_sub(4);
            for wrapped_line in wrap_words(line, max_width) {
                content.push(Line::from(Span::styled(
                    format!("  {wrapped_line}"),
                    Style::default().fg(theme.colors.error),
                )));
            }
        }
    }
}

fn build_message_section<'a>(
    content: &mut Vec<Line<'a>>,
    record: &'a crate::events::RecordSummary,
    theme: &Theme,
) {
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "Message:",
        Style::default().fg(theme.colors.muted),
    )));

    let base_style = Style::default().fg(theme.colors.foreground);
    for line in record.message.lines() {
        if line.is_empty() {
            content.push(Line::from(""));
        } else {
            let spans = parse_markdown_line(line, base_style, theme);
            content.push(Line::from(spans));
        }
    }
}

/// Render a notification.
fn render_notification(
    frame: &mut Frame,
    msg: &str,
    notification_type: NotificationType,
    theme: &Theme,
) {
    let area = frame.area();
    let msg_width = u16::try_from(display_width(msg)).unwrap_or(u16::MAX);
    let toast_width = msg_width
        .saturating_add(6)
        .min(area.width.saturating_sub(4));

    let toast_area = Rect {
        x: area.width.saturating_sub(toast_width + 1),
        y: 0,
        width: toast_width,
        height: 1,
    };

    let (icon, color) = match notification_type {
        NotificationType::Success => ("✓", theme.colors.success),
        NotificationType::Warning => ("!", theme.colors.warning),
        NotificationType::Error => ("✗", theme.colors.error),
    };

    let content = Line::from(vec![
        Span::styled(format!(" {icon} "), Style::default().fg(color)),
        Span::styled(msg, Style::default().fg(theme.colors.foreground)),
    ]);

    let paragraph = Paragraph::new(content);
    frame.render_widget(paragraph, toast_area);
}
