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

    if app.state() == AppState::SelectingModel {
        render_model_selector(frame, app, theme);
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

/// Render the model selector, grouped into local and cloud models.
fn render_model_selector(frame: &mut Frame, app: &App, theme: &Theme) {
    let models = app.models();
    let guest = app.user().is_none();
    let mut lines: Vec<Line<'_>> = Vec::new();
    let mut last_group: Option<bool> = None;

    for (idx, model) in models.iter().enumerate() {
        if last_group != Some(model.local) {
            if last_group.is_some() {
                lines.push(Line::from(""));
            }
            let heading = if model.local {
                "Local (no login)"
            } else {
                "Cloud (aura-router)"
            };
            lines.push(Line::from(Span::styled(
                heading,
                Style::default().fg(theme.colors.muted),
            )));
            last_group = Some(model.local);
        }

        let selected = idx == app.model_cursor();
        let current = model.id == app.current_model();
        let marker = if selected { "› " } else { "  " };
        let check = if current { "● " } else { "  " };
        let note = if model.local && !model.installed {
            "  not pulled"
        } else if !model.local && guest {
            "  login required"
        } else {
            ""
        };
        let label_color = if selected {
            theme.colors.primary
        } else {
            theme.colors.foreground
        };
        lines.push(Line::from(vec![
            Span::styled(marker, Style::default().fg(theme.colors.primary)),
            Span::styled(check, Style::default().fg(theme.colors.success)),
            Span::styled(model.label.clone(), Style::default().fg(label_color)),
            Span::styled(note, Style::default().fg(theme.colors.warning)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑/↓ move  Enter select  Esc close",
        Style::default().fg(theme.colors.muted),
    )));

    let area = frame.area();
    let modal_width = 52.min(area.width.saturating_sub(4));
    #[expect(
        clippy::cast_possible_truncation,
        reason = "selector line count is small"
    )]
    let modal_height = ((lines.len() + 2) as u16).min(area.height.saturating_sub(2));
    let modal_area = centered_rect(modal_width, modal_height, area);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" Select model ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.primary));
    frame.render_widget(Paragraph::new(lines).block(block), modal_area);
}

/// Render the help overlay.
fn render_help_overlay(frame: &mut Frame, theme: &Theme) {
    let area = frame.area();
    let modal_width = 50.min(area.width.saturating_sub(4));
    let modal_height = 27;

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
        "/agent new [name]   Create and switch to agent",
        "/agent <n|name>     Switch to agent",
        "/agents    Toggle Agents panel",
        "/record    Toggle Record panel",
        "/files     Toggle Sandbox panel",
        "/model     Choose model (local or cloud)",
        "/login     Login for cloud models",
        "/logout    Clear credentials",
        "/whoami     Show auth status",
        "/quit      Exit",
        "",
        "Enter      Send message",
        "Tab        Switch panels",
        "↑/↓        Navigate / History",
        "Ctrl+N     New agent",
        "Alt+↑/↓    Switch agent",
        "Mouse      Wheel/click agents to switch",
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
    #[allow(clippy::cast_possible_truncation)]
    let policy_lines = record
        .policy
        .as_ref()
        .map_or(0, |policy| 10 + policy.trace.len() as u16);
    let modal_height =
        (base_height + error_lines + policy_lines).min(area.height.saturating_sub(4));

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
    if let Some(policy) = &record.policy {
        build_policy_section(&mut content, policy, theme);
    }

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

fn build_policy_section<'a>(
    content: &mut Vec<Line<'a>>,
    policy: &'a crate::events::PolicyDetail,
    theme: &Theme,
) {
    let label = |text: &'static str| Span::styled(text, Style::default().fg(theme.colors.muted));
    let verdict_color = match policy.verdict.as_str() {
        "approved" => theme.colors.success,
        "denied" => theme.colors.error,
        _ => theme.colors.pending,
    };

    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "── Policy decision ──",
        Style::default().fg(theme.colors.muted),
    )));
    content.push(Line::from(vec![
        label("Tool:        "),
        Span::styled(&policy.tool, Style::default().fg(theme.colors.foreground)),
    ]));
    content.push(Line::from(vec![
        label("Verdict:     "),
        Span::styled(&policy.verdict, Style::default().fg(verdict_color)),
    ]));
    content.push(Line::from(vec![
        label("Decided by:  "),
        Span::styled(
            &policy.decided_by,
            Style::default().fg(theme.colors.primary),
        ),
    ]));
    if !policy.reason.is_empty() {
        content.push(Line::from(vec![
            label("Reason:      "),
            Span::styled(&policy.reason, Style::default().fg(theme.colors.foreground)),
        ]));
    }
    if !policy.approval.is_empty() {
        content.push(Line::from(vec![
            label("Approved by: "),
            Span::styled(
                &policy.approval,
                Style::default().fg(theme.colors.foreground),
            ),
        ]));
    }
    content.push(Line::from(vec![
        label("Mode:        "),
        Span::styled(&policy.mode, Style::default().fg(theme.colors.foreground)),
    ]));
    content.push(Line::from(vec![
        label("Proposal tx: "),
        Span::styled(
            &policy.proposal_tx,
            Style::default().fg(theme.colors.secondary),
        ),
    ]));
    content.push(Line::from(vec![
        label("Policy hash: "),
        Span::styled(
            &policy.policy_hash,
            Style::default().fg(theme.colors.secondary),
        ),
    ]));
    if !policy.trace.is_empty() {
        content.push(Line::from(label("Checks:")));
        for check in &policy.trace {
            content.push(Line::from(Span::styled(
                format!("  {check}"),
                Style::default().fg(theme.colors.foreground),
            )));
        }
    }
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
