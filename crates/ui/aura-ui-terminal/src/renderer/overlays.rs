//! Overlay rendering: approval modal, help, notifications, and the lines
//! of a record's details.

use super::markdown::{parse_content_segments, render_markdown, ContentSegment};
use super::text::{centered_rect, display_width, wrap_words};
use crate::{
    app::{folder_picker::FolderRow, AppState, NotificationType},
    components::{highlight_code_line, CodeBlock},
    App, Theme,
};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};
use std::path::Path;

/// Render overlay elements (modals, help).
pub(super) fn render_overlays(frame: &mut Frame, app: &App, theme: &Theme) {
    if let Some(approval) = app.pending_approval() {
        render_approval_modal(frame, approval, app.pending_approval_count(), theme);
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

    if let Some((msg, kind)) = app.copy_notice() {
        render_copy_notice(frame, app, msg, kind, theme);
    }
}

/// Render the approval modal.
fn render_approval_modal(
    frame: &mut Frame,
    approval: &crate::app::PendingApproval,
    waiting: usize,
    theme: &Theme,
) {
    /// Most lines of the action's description shown.
    const MAX_DESCRIPTION_LINES: usize = 6;

    let area = frame.area();
    let modal_width = 64.min(area.width.saturating_sub(4));
    let text_width = usize::from(modal_width.saturating_sub(2)).max(1);
    let muted = Style::default().fg(theme.colors.muted);

    let mut content = vec![Line::from(Span::styled(
        format!("{} wants to:", approval.tool),
        Style::default().fg(theme.colors.foreground),
    ))];
    let description = wrap_words(&approval.description, text_width);
    let cut = description.len() > MAX_DESCRIPTION_LINES;
    content.extend(
        description
            .into_iter()
            .take(MAX_DESCRIPTION_LINES)
            .map(|line| Line::from(Span::styled(line, muted))),
    );
    if cut {
        content.push(Line::from(Span::styled("…", muted)));
    }
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "[Y] Allow once",
        Style::default().fg(theme.colors.success),
    )));
    if !approval.always.is_empty() {
        content.push(Line::from(Span::styled(
            format!("[A] Always allow {}", approval.always),
            Style::default().fg(theme.colors.primary),
        )));
    }
    content.push(Line::from(Span::styled(
        "[N] Deny (Esc)",
        Style::default().fg(theme.colors.error),
    )));

    let height = u16::try_from(content.len() + 2).unwrap_or(u16::MAX);
    let modal_area = centered_rect(modal_width, height.min(area.height), area);
    frame.render_widget(Clear, modal_area);

    let title = if waiting > 1 {
        format!(" Approval Required (1 of {waiting}) ")
    } else {
        " Approval Required ".to_string()
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.warning));
    let paragraph = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, modal_area);
}

/// Render the model selector, grouped by route (local, cloud, plans).
fn render_model_selector(frame: &mut Frame, app: &App, theme: &Theme) {
    let models = app.models();
    let mut lines: Vec<Line<'_>> = Vec::new();
    let mut last_group: Option<&str> = None;

    for (idx, model) in models.iter().enumerate() {
        if last_group != Some(model.group.as_str()) {
            if last_group.is_some() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                model.group.as_str(),
                Style::default().fg(theme.colors.muted),
            )));
            last_group = Some(model.group.as_str());
        }

        let selected = idx == app.model_cursor();
        let current = model.id == app.current_model();
        let marker = if selected { "› " } else { "  " };
        let check = if current { "● " } else { "  " };
        let note = if model.local && !model.installed {
            "  not pulled".to_string()
        } else if model.available {
            String::new()
        } else {
            match model.provider.as_deref() {
                Some(provider) if provider != "zos" => format!("  /login {provider}"),
                _ => "  login required".to_string(),
            }
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

/// Render the new-project folder browser: the directory being browsed,
/// its subfolders, and the keys that drive it.
pub(super) fn render_folder_picker(frame: &mut Frame, app: &mut App, theme: &Theme) {
    if app.state() != AppState::SelectingFolder {
        return;
    }
    let area = frame.area();
    let modal_width = 70.min(area.width.saturating_sub(4));
    let modal_height = 24.min(area.height.saturating_sub(2));
    let modal_area = centered_rect(modal_width, modal_height, area);
    let inner_width = usize::from(modal_width.saturating_sub(2));
    // Path line, blank line above the footer, and two footer lines.
    let fixed_rows = 4 + usize::from(
        app.folder_picker()
            .is_some_and(|p| p.new_folder().is_some()),
    );
    let visible = usize::from(modal_height.saturating_sub(2)).saturating_sub(fixed_rows);
    let scroll = app.sync_folder_picker_scroll(visible);
    let Some(picker) = app.folder_picker() else {
        return;
    };

    let muted = Style::default().fg(theme.colors.muted);
    let path = tilde_path(picker.dir());
    let mut lines = vec![Line::from(Span::styled(
        truncate_start(&path, inner_width),
        Style::default().fg(theme.colors.secondary),
    ))];

    for (index, row) in picker.rows().iter().enumerate().skip(scroll).take(visible) {
        let selected = index == picker.selected();
        let marker = Span::styled(
            if selected { "› " } else { "  " },
            Style::default().fg(theme.colors.primary),
        );
        let label_style = |color| {
            Style::default().fg(if selected {
                theme.colors.primary
            } else {
                color
            })
        };
        let line = match row {
            FolderRow::UseThis => Line::from(vec![
                marker,
                Span::styled("✓ ", Style::default().fg(theme.colors.success)),
                Span::styled("Use this folder", label_style(theme.colors.success)),
            ]),
            FolderRow::Parent => Line::from(vec![
                marker,
                Span::styled("  ", muted),
                Span::styled("..", label_style(theme.colors.foreground)),
            ]),
            FolderRow::Folder(name) => Line::from(vec![
                marker,
                Span::styled("▰ ", muted),
                Span::styled(name.as_str(), label_style(theme.colors.foreground)),
            ]),
            FolderRow::More(hidden) => Line::from(vec![
                marker,
                Span::styled(format!("  … {hidden} more"), muted),
            ]),
        };
        lines.push(line);
    }
    let shown = lines.len();
    lines.extend(std::iter::repeat_n(
        Line::from(""),
        (visible + 1).saturating_sub(shown),
    ));

    if let Some(name) = picker.new_folder() {
        lines.push(Line::from(vec![
            Span::styled("New folder: ", Style::default().fg(theme.colors.primary)),
            Span::styled(name, Style::default().fg(theme.colors.foreground)),
            Span::styled("█", Style::default().fg(theme.colors.primary)),
        ]));
    }
    lines.push(Line::from(""));
    if picker.new_folder().is_some() {
        lines.push(Line::from(Span::styled("Enter create  Esc stop", muted)));
        lines.push(Line::from(""));
    } else {
        let hidden = if picker.show_hidden() { "hide" } else { "show" };
        lines.push(Line::from(Span::styled(
            "↑/↓ move  Enter/→ open  ←/Backspace up  Space pick highlighted",
            muted,
        )));
        lines.push(Line::from(Span::styled(
            format!("n new folder  ~ home  . {hidden} hidden  Esc cancel"),
            muted,
        )));
    }

    frame.render_widget(Clear, modal_area);
    let block = Block::default()
        .title(" Select project folder ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.primary));
    frame.render_widget(Paragraph::new(lines).block(block), modal_area);
}

/// `path` with the home directory shown as `~`.
fn tilde_path(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// `text` cut from the left with `…` so it fits `width` columns.
fn truncate_start(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 1;
    for c in text.chars().rev() {
        let w = display_width(c.encode_utf8(&mut [0; 4]));
        if used + w > width {
            break;
        }
        used += w;
        kept.push(c);
    }
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}

/// Render the help overlay.
fn render_help_overlay(frame: &mut Frame, theme: &Theme) {
    let area = frame.area();
    let modal_width = 50.min(area.width.saturating_sub(4));
    let help_text = build_help_lines(theme);
    // Every line plus the two border rows, so nothing is clipped on a
    // screen tall enough to hold it.
    let modal_height = u16::try_from(help_text.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(area.height);

    let modal_area = centered_rect(modal_width, modal_height, area);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.primary));

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
        "/resume [n] List or resume earlier sessions",
        "/rewind [n] [code|chat]  Undo to before turn n",
        "/diff [session]     Files changed by last turn",
        "/gate /compare /good /bad [n]  Gate, A/B, rate",
        "/clear     Clear messages",
        "/context   What fills the context window",
        "/compact [focus]    Summarize older history now",
        "/init      Write or improve AGENTS.md",
        "/todos     Toggle the agent's todo list",
        "/cost      Tokens and cost of the session",
        "/agent [n|name]     Add an agent, or switch",
        "/agent new [name]   Create and switch to agent",
        "/name [name]        Rename the current project",
        "/agents /kernel /files  Toggle those panels",
        "/graph [id] /runs /status  Run graphs, scores",
        "/model     Choose model (local or cloud)",
        "/mode [name] /approval [level]  Work, what asks",
        "/plan      Enter or leave plan mode",
        "/build [path]  Implement the plan",
        "/login /whoami  Login for cloud models, status",
        "/login claude|chatgpt [paste]  Link a plan",
        "/logout [claude|chatgpt]  Sign out / unlink",
        "/quit      Exit",
        "",
        "Enter      Send message",
        "@path      Attach a file or folder",
        "Ctrl+V     Paste an image (or text)",
        "Ctrl+G     Edit the input in $EDITOR",
        "$ / Esc    Open the terminal / back to chat",
        "Tab        Switch panels",
        "Shift+Tab  Approval level (yolo→ask→strict)",
        "↑/↓        Navigate / History",
        "Ctrl+N     New project (pick its folder)",
        "Space      Fold the selected project",
        "Shift+←/→  Sandbox / Processes tabs",
        "o / k / l  Process: open agent, kill, log",
        "Alt+↑/↓    Switch agent",
        "Mouse      Wheel/click agents to switch",
        "PgUp/PgDn  Scroll chat history",
        "Ctrl+Home/End  Chat top/bottom",
        "Ctrl+O     Expand/collapse thinking & tools",
        "Click      Open a summary, then a step's details",
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

/// Status glyph and color shown in a record's title.
pub(super) const fn record_status(
    record: &crate::events::RecordSummary,
    theme: &Theme,
) -> (&'static str, Color) {
    use crate::events::RecordStatus;
    match record.status {
        RecordStatus::Ok => ("✓", theme.colors.success),
        RecordStatus::Error => ("✗", theme.colors.error),
        RecordStatus::Pending => ("◌", theme.colors.pending),
        RecordStatus::None => ("·", theme.colors.muted),
    }
}

/// Every line of a record's details, with prose wrapped to `width`.
pub(super) fn record_detail_lines<'a>(
    record: &'a crate::events::RecordSummary,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'a>> {
    let mut content = build_record_header(record, theme);
    build_transaction_section(&mut content, record, theme);
    build_processing_section(&mut content, record, theme);
    if let Some(policy) = &record.policy {
        build_policy_section(&mut content, policy, theme);
    }
    if !record.error_details.is_empty() {
        build_error_section(&mut content, record, width, theme);
    }
    build_message_section(&mut content, record, width, theme);
    content
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

fn build_message_section(
    content: &mut Vec<Line<'_>>,
    record: &crate::events::RecordSummary,
    modal_width: u16,
    theme: &Theme,
) {
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "Message:",
        Style::default().fg(theme.colors.muted),
    )));

    if record.message.starts_with(['{', '[']) {
        content.extend(
            record
                .message
                .lines()
                .map(|line| Line::from(highlight_code_line("json", line, theme))),
        );
        return;
    }

    let base_style = Style::default().fg(theme.colors.foreground);
    let width = usize::from(modal_width.saturating_sub(2));
    for segment in parse_content_segments(&record.message) {
        match segment {
            ContentSegment::Text(text) => {
                content.extend(render_markdown(&text, width, base_style, theme));
            }
            ContentSegment::CodeBlock { language, code } => {
                content.extend(CodeBlock::new(language, code).render(theme, width));
            }
        }
    }
}

/// Render a notification over the right end of the header row.
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

    let (icon, color) = notification_icon(notification_type, theme);
    let content = Line::from(vec![
        Span::styled(format!(" {icon} "), Style::default().fg(color)),
        Span::styled(msg, Style::default().fg(theme.colors.foreground)),
    ]);

    frame.render_widget(Clear, toast_area);
    frame.render_widget(Paragraph::new(content), toast_area);
}

const fn notification_icon(kind: NotificationType, theme: &Theme) -> (&'static str, Color) {
    match kind {
        NotificationType::Success => ("✓", theme.colors.success),
        NotificationType::Warning => ("!", theme.colors.warning),
        NotificationType::Error => ("✗", theme.colors.error),
    }
}

/// `✓ Copied 4 characters`, centered on the `AURA CLI` row of the header.
/// Skipped when it would touch the title or the status.
fn render_copy_notice(
    frame: &mut Frame,
    app: &App,
    msg: &str,
    notification_type: NotificationType,
    theme: &Theme,
) {
    let area = frame.area();
    if area.height == 0 {
        return;
    }

    let (icon, color) = notification_icon(notification_type, theme);
    let content = Line::from(vec![
        Span::styled(format!("{icon} "), Style::default().fg(color)),
        Span::styled(msg, Style::default().fg(theme.colors.foreground)),
    ]);
    let Ok(width) = u16::try_from(content.width()) else {
        return;
    };

    let to_u16 = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
    let title_end = area.x + super::EDGE_INSET + to_u16(display_width("AURA CLI"));
    let status_width = super::spans_width(&super::header_status_spans(app, theme));
    let status_start =
        (area.x + area.width).saturating_sub(super::EDGE_INSET + to_u16(status_width));
    let (min_x, max_end) = (title_end + 1, status_start.saturating_sub(1));
    if max_end.saturating_sub(min_x) < width {
        return;
    }
    let centered = area.x + (area.width - width) / 2;
    let notice = Rect {
        x: centered.clamp(min_x, max_end - width),
        y: area.y,
        width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(content), notice);
}
