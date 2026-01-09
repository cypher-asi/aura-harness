//! Simple IRC-style renderer for the terminal UI.

use crate::{
    app::{AppState, NotificationType, PanelFocus},
    components::MessageRole,
    App, Theme,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

/// Render the full application UI in IRC style.
pub fn render(frame: &mut Frame, app: &App, theme: &Theme) {
    let area = frame.area();

    // Layout: header + content panels + input line
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header (with padding)
            Constraint::Min(3),    // Content area (Chat panel + optional Record panel)
            Constraint::Length(1), // Input line (with status on right)
        ])
        .split(area);

    // Render header
    render_header(frame, main_chunks[0], theme);

    // Render panels based on Record panel visibility
    if app.record_panel_visible() {
        // Split content area horizontally: Chat panel (65%) | Record panel (35%)
        let content_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(65), // Chat panel
                Constraint::Percentage(35), // Record panel
            ])
            .split(main_chunks[1]);

        render_chat_panel(frame, content_chunks[0], app, theme);
        render_record_panel(frame, content_chunks[1], app, theme);
    } else {
        // Only Chat panel (full width)
        render_chat_panel(frame, main_chunks[1], app, theme);
    }

    // Render input line with status on right
    render_input(frame, main_chunks[2], app, theme);

    // Render overlays (approval modal, help, record detail)
    render_overlays(frame, app, theme);
}

/// Render the header bar.
fn render_header(frame: &mut Frame, area: Rect, theme: &Theme) {
    let header = vec![
        Line::from(""),
        Line::from(Span::styled("AURA OS", Style::default().fg(theme.colors.primary))),
        Line::from(""),
    ];
    frame.render_widget(Paragraph::new(header), area);
}

/// Render the Chat panel.
fn render_chat_panel(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Chat;
    let border_color = if is_focused {
        theme.colors.primary
    } else {
        theme.colors.muted
    };

    let block = Block::default()
        .title(" Chat ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Add padding: 2 chars left/right, 1 line top
    let padded = Rect {
        x: inner.x.saturating_add(2),
        y: inner.y.saturating_add(1),
        width: inner.width.saturating_sub(4),
        height: inner.height.saturating_sub(1),
    };

    let messages = app.messages();

    if messages.is_empty() {
        // Show simple welcome
        let welcome = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Type a message to start chatting, or /help for commands.",
                Style::default().fg(theme.colors.muted),
            )),
        ];
        let paragraph = Paragraph::new(welcome);
        frame.render_widget(paragraph, padded);
        return;
    }

    // Build IRC-style message lines with proper word wrapping
    let mut lines: Vec<Line> = Vec::new();
    let content_width = padded.width as usize;

    for message in messages.iter().skip(app.scroll_offset()) {
        // Format: [HH:MM:SS] <NICK> message
        // User: <YOU> in white, message in light gray
        // AURA: <AURA> in white, message in neon cyan
        let (nick, nick_color, msg_color) = match message.role() {
            MessageRole::User => ("YOU", theme.colors.foreground, theme.colors.muted),
            MessageRole::Assistant => ("AURA", theme.colors.foreground, theme.colors.primary),
            MessageRole::System => ("*", theme.colors.muted, theme.colors.muted),
        };

        // Use the message's stored timestamp
        let timestamp = message.timestamp_local();

        // Calculate prefix width: "[HH:MM:SS] <NICK> " 
        // "[HH:MM:SS] " = 11 chars, "<NICK>" = nick.len() + 2 chars, " " = 1 char
        let prefix_width = 11 + nick.len() + 2 + 1; // e.g., "[12:34:56] <YOU> " = 17, "<AURA> " = 18

        // Available width for message content on first line
        let first_line_width = content_width.saturating_sub(prefix_width);
        // Continuation lines are indented to align with message text
        let continuation_width = content_width.saturating_sub(prefix_width);

        let mut is_first_output_line = true;
        for content_line in message.content().lines() {
            if content_line.is_empty() {
                // Empty line - just add the prefix or indent
                if is_first_output_line {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("[{timestamp}] "),
                            Style::default().fg(theme.colors.muted),
                        ),
                        Span::styled(format!("<{nick}>"), Style::default().fg(nick_color)),
                    ]));
                    is_first_output_line = false;
                } else {
                    lines.push(Line::from(""));
                }
                continue;
            }

            // Word-wrap the content line
            let wrap_width = if is_first_output_line { first_line_width } else { continuation_width };
            let wrapped = wrap_words(content_line, wrap_width);

            for wrapped_line in wrapped {
                if is_first_output_line {
                    // First line of first content line: include timestamp and nick
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("[{timestamp}] "),
                            Style::default().fg(theme.colors.muted),
                        ),
                        Span::styled(format!("<{nick}>"), Style::default().fg(nick_color)),
                        Span::raw(" "),
                        Span::styled(wrapped_line, Style::default().fg(msg_color)),
                    ]));
                    is_first_output_line = false;
                } else {
                    // Continuation: indent to align with message text
                    let indent = " ".repeat(prefix_width);
                    lines.push(Line::from(vec![
                        Span::raw(indent),
                        Span::styled(wrapped_line, Style::default().fg(msg_color)),
                    ]));
                }
            }
        }
    }

    // Scroll to show most recent messages at the bottom
    let visible_height = padded.height as usize;
    let start = lines.len().saturating_sub(visible_height);
    let visible_lines: Vec<Line> = lines.into_iter().skip(start).collect();

    let paragraph = Paragraph::new(visible_lines);
    frame.render_widget(paragraph, padded);
}

/// Wrap text at word boundaries to fit within max_width.
/// Returns a vector of lines, each fitting within the width.
fn wrap_words(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0;

    for word in text.split_whitespace() {
        let word_len = word.chars().count();

        if current_width == 0 {
            // First word on line
            if word_len > max_width {
                // Word is longer than max_width, need to break it
                let mut chars = word.chars().peekable();
                while chars.peek().is_some() {
                    let chunk: String = chars.by_ref().take(max_width).collect();
                    if chars.peek().is_some() {
                        // More chars remaining, push this chunk as complete line
                        lines.push(chunk);
                    } else {
                        // Last chunk, make it the current line
                        current_width = chunk.chars().count();
                        current_line = chunk;
                    }
                }
            } else {
                current_line = word.to_string();
                current_width = word_len;
            }
        } else if current_width + 1 + word_len <= max_width {
            // Word fits on current line with space
            current_line.push(' ');
            current_line.push_str(word);
            current_width += 1 + word_len;
        } else {
            // Word doesn't fit, start new line
            lines.push(std::mem::take(&mut current_line));
            current_width = 0;

            if word_len > max_width {
                // Word is longer than max_width, need to break it
                let mut chars = word.chars().peekable();
                while chars.peek().is_some() {
                    let chunk: String = chars.by_ref().take(max_width).collect();
                    if chars.peek().is_some() {
                        // More chars remaining, push this chunk as complete line
                        lines.push(chunk);
                    } else {
                        // Last chunk, make it the current line
                        current_width = chunk.chars().count();
                        current_line = chunk;
                    }
                }
            } else {
                current_line = word.to_string();
                current_width = word_len;
            }
        }
    }

    // Don't forget the last line
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    // If no lines were created (empty or whitespace-only input), return single empty line
    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Render the Record panel.
fn render_record_panel(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Records;
    let border_color = if is_focused {
        theme.colors.primary
    } else {
        theme.colors.muted
    };

    let block = Block::default()
        .title(" Record ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let records = app.records();

    if records.is_empty() {
        let empty_msg = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No records yet.",
                Style::default().fg(theme.colors.muted),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Records will appear",
                Style::default().fg(theme.colors.muted),
            )),
            Line::from(Span::styled(
                "  as they are created.",
                Style::default().fg(theme.colors.muted),
            )),
        ];
        let paragraph = Paragraph::new(empty_msg);
        frame.render_widget(paragraph, inner);
        return;
    }

    // Build record list - one line per record
    // Format: # | time | ...hash | type
    let selected = app.selected_record();
    let mut lines: Vec<Line> = Vec::new();

    for (i, record) in records.iter().enumerate() {
        let is_selected = i == selected;
        let prefix = if is_selected && is_focused { ">" } else { " " };

        let line_style = if is_selected && is_focused {
            Style::default()
                .fg(theme.colors.primary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.colors.muted)
        };

        let hash_style = if is_selected && is_focused {
            Style::default().fg(theme.colors.secondary)
        } else {
            Style::default().fg(theme.colors.muted)
        };

        lines.push(Line::from(vec![
            Span::styled(prefix, line_style),
            Span::styled(format!("#{:<3}", record.seq), line_style),
            Span::styled(format!(" {} ", record.timestamp), line_style),
            Span::styled(format!("...{} ", record.hash_suffix), hash_style),
            Span::styled(&record.tx_kind, line_style),
        ]));
    }

    // Handle scrolling for records
    let visible_height = inner.height as usize;

    // Calculate scroll offset to keep selected item visible
    let scroll = if selected >= visible_height {
        selected.saturating_sub(visible_height / 2)
    } else {
        0
    };

    let visible_lines: Vec<Line> = lines.into_iter().skip(scroll).collect();

    let paragraph = Paragraph::new(visible_lines);
    frame.render_widget(paragraph, inner);
}

/// Render input line with status on the right.
fn render_input(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let input = app.input();
    let cursor_pos = app.cursor_pos();

    // Neon cyan prompt
    let prompt_color = theme.colors.primary;

    // Build status indicator for right side
    let status = app.status();
    let is_ready = status == "Ready";
    let is_thinking = status.contains("Thinking");

    // Determine status style: Ready = cyan, Thinking = gray with spinner, other = warning
    let status_style = if is_ready {
        Style::default().fg(theme.colors.primary)
    } else if is_thinking {
        Style::default().fg(theme.colors.muted)
    } else {
        Style::default().fg(theme.colors.warning)
    };

    // Use spinner for thinking, solid dot for ready, half-moon for other
    let status_icon = if is_ready {
        "●"
    } else if is_thinking {
        app.spinner_char()
    } else {
        "◐"
    };
    let status_text = format!("{} {}", status_icon, status);
    // Use char count, not byte length (unicode chars like ⠙ are multi-byte)
    let status_len = status_text.chars().count() as u16;

    // Calculate available width for input (leave space for status on right)
    let input_width = area.width.saturating_sub(status_len + 2);

    // Render status on the right
    let status_area = Rect {
        x: area.x + area.width.saturating_sub(status_len),
        y: area.y,
        width: status_len,
        height: 1,
    };
    let status_line = Line::from(Span::styled(&status_text, status_style));
    frame.render_widget(Paragraph::new(status_line), status_area);

    // Build input line (no fake cursor - we use the real terminal cursor)
    let input_area = Rect {
        x: area.x,
        y: area.y,
        width: input_width,
        height: 1,
    };

    let content = Line::from(vec![
        Span::styled("> ", Style::default().fg(prompt_color)),
        Span::styled(input, Style::default().fg(theme.colors.muted)),
    ]);

    frame.render_widget(Paragraph::new(content), input_area);

    // Only show the blinking cursor when ready for input (not during thinking)
    if is_ready {
        // Position the native terminal cursor (it blinks automatically)
        // Prompt "> " is 2 chars, then cursor_pos chars into the input
        let cursor_x = area.x + 2 + cursor_pos as u16;
        let cursor_y = area.y;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

/// Render overlay elements (modals, help).
fn render_overlays(frame: &mut Frame, app: &App, theme: &Theme) {
    // Record detail overlay
    if app.showing_record_detail() {
        render_record_detail(frame, app, theme);
    }

    // Approval modal
    if let Some(approval) = app.pending_approval() {
        render_approval_modal(frame, approval, theme);
    }

    // Help overlay
    if app.state() == AppState::ShowingHelp {
        render_help_overlay(frame, theme);
    }

    // Notification
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
        Line::from(vec![
            Span::styled(
                format!("{} wants to: ", approval.tool),
                Style::default().fg(theme.colors.foreground),
            ),
        ]),
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
    let modal_height = 17;

    let modal_area = centered_rect(modal_width, modal_height, area);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.primary));

    let help_text = vec![
        Line::from(Span::styled(
            "/help      Show this help",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "/new       New session (reset context)",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "/clear     Clear messages",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "/record    Toggle Record panel",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "/quit      Exit",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Enter      Send message",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "Tab        Switch panels",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "↑/↓        Navigate / History",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "Ctrl+C     Cancel/Exit",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press any key to close",
            Style::default().fg(theme.colors.muted),
        )),
    ];

    let paragraph = Paragraph::new(help_text)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, modal_area);
}

/// Render the record detail overlay.
fn render_record_detail(frame: &mut Frame, app: &App, theme: &Theme) {
    let Some(record) = app.selected_record_data() else {
        return;
    };

    let area = frame.area();
    let modal_width = 60.min(area.width.saturating_sub(4));
    let modal_height = 16.min(area.height.saturating_sub(4));

    let modal_area = centered_rect(modal_width, modal_height, area);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(format!(" Record #{} ", record.seq))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.primary));

    let content = vec![
        Line::from(vec![
            Span::styled("Sequence:  ", Style::default().fg(theme.colors.muted)),
            Span::styled(
                format!("{}", record.seq),
                Style::default().fg(theme.colors.foreground),
            ),
        ]),
        Line::from(vec![
            Span::styled("Timestamp: ", Style::default().fg(theme.colors.muted)),
            Span::styled(&record.timestamp, Style::default().fg(theme.colors.foreground)),
        ]),
        Line::from(vec![
            Span::styled("Hash:      ", Style::default().fg(theme.colors.muted)),
            Span::styled(&record.full_hash, Style::default().fg(theme.colors.secondary)),
        ]),
        Line::from(vec![
            Span::styled("TX Kind:   ", Style::default().fg(theme.colors.muted)),
            Span::styled(&record.tx_kind, Style::default().fg(theme.colors.foreground)),
        ]),
        Line::from(vec![
            Span::styled("Sender:    ", Style::default().fg(theme.colors.muted)),
            Span::styled(&record.sender, Style::default().fg(theme.colors.foreground)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Actions:   ", Style::default().fg(theme.colors.muted)),
            Span::styled(
                format!("{}", record.action_count),
                Style::default().fg(theme.colors.secondary),
            ),
        ]),
        Line::from(vec![
            Span::styled("Effects:   ", Style::default().fg(theme.colors.muted)),
            Span::styled(
                &record.effect_status,
                Style::default().fg(theme.colors.foreground),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled("Message:", Style::default().fg(theme.colors.muted))),
        Line::from(Span::styled(
            &record.message,
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press Esc or Enter to close",
            Style::default().fg(theme.colors.muted),
        )),
    ];

    let paragraph = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, modal_area);
}

/// Render a notification.
fn render_notification(
    frame: &mut Frame,
    msg: &str,
    notification_type: NotificationType,
    theme: &Theme,
) {
    let area = frame.area();
    let msg_len = u16::try_from(msg.len()).unwrap_or(u16::MAX);
    let toast_width = msg_len.saturating_add(6).min(area.width.saturating_sub(4));

    // Position at top-right
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

/// Helper to create a centered rect.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}
