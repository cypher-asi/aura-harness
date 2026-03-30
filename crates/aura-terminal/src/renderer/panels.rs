//! Panel rendering: Chat, Swarm, Record, and Thinking section.

use super::markdown::{parse_content_segments, parse_markdown_line, ContentSegment};
use super::text::{display_width, wrap_words};
use crate::{
    app::PanelFocus,
    components::{CodeBlock, MessageRole},
    App, Theme,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// Render the thinking section at the bottom of the chat panel.
pub(super) fn render_thinking_section(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let thinking_content = app.thinking_content();
    let is_thinking = app.is_thinking();

    let divider_width = area.width as usize;

    let label = if is_thinking {
        format!(" Thinker  {}  ", app.spinner_char())
    } else {
        " Thinker ".to_string()
    };
    let label_len = label.chars().count();
    let right_dashes = divider_width.saturating_sub(label_len);

    let divider_spans = vec![
        Span::styled(label, Style::default().fg(theme.colors.muted)),
        Span::styled(
            "─".repeat(right_dashes),
            Style::default().fg(theme.colors.muted),
        ),
    ];

    let content_lines: Vec<String> = if thinking_content.is_empty() {
        if is_thinking {
            vec!["...".to_string()]
        } else {
            vec![]
        }
    } else {
        let max_width = area.width.saturating_sub(2) as usize;
        let mut wrapped_lines: Vec<String> = Vec::new();

        for line in thinking_content.lines() {
            if line.is_empty() {
                wrapped_lines.push(String::new());
            } else {
                wrapped_lines.extend(wrap_words(line, max_width));
            }
        }

        let start = wrapped_lines.len().saturating_sub(3);
        wrapped_lines.into_iter().skip(start).collect()
    };

    let mut lines = vec![Line::from(divider_spans)];

    for line_text in content_lines.iter().take(3) {
        lines.push(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                line_text.clone(),
                Style::default()
                    .fg(theme.colors.muted)
                    .add_modifier(Modifier::DIM),
            ),
        ]));
    }

    while lines.len() < 4 {
        lines.push(Line::from(""));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Render the Chat panel.
#[expect(
    clippy::too_many_lines,
    reason = "UI rendering function with many visual components"
)]
pub(super) fn render_chat_panel(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
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

    let thinking_section_height = 4u16;
    let show_thinking_section = app.is_thinking() || !app.thinking_content().is_empty();

    let (messages_area, thinking_area) =
        if show_thinking_section && inner.height > thinking_section_height + 3 {
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(3),
                    Constraint::Length(thinking_section_height),
                ])
                .split(inner);
            (split[0], Some(split[1]))
        } else {
            (inner, None)
        };

    if let Some(think_area) = thinking_area {
        render_thinking_section(frame, think_area, app, theme);
    }

    let padded = Rect {
        x: messages_area.x.saturating_add(2),
        y: messages_area.y.saturating_add(1),
        width: messages_area.width.saturating_sub(4),
        height: messages_area.height.saturating_sub(1),
    };

    let messages = app.messages();

    if messages.is_empty() {
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

    let mut lines: Vec<Line> = Vec::new();
    let content_width = padded.width as usize;

    for message in messages {
        let (nick, nick_color, msg_color) = match message.role() {
            MessageRole::User => ("YOU", theme.colors.foreground, theme.colors.primary),
            MessageRole::Assistant => ("AURA", theme.colors.foreground, theme.colors.muted),
            MessageRole::System => {
                let content = message.content();
                if content.starts_with("⛔") || content.contains("Error") {
                    ("*", theme.colors.error, theme.colors.error)
                } else if content.starts_with("⚠") || content.contains("Warning") {
                    ("*", theme.colors.warning, theme.colors.warning)
                } else if content.starts_with("✓") {
                    ("*", theme.colors.success, theme.colors.success)
                } else {
                    ("*", theme.colors.muted, theme.colors.muted)
                }
            }
        };

        let timestamp = message.timestamp_local();
        let prefix_width = 11 + nick.len() + 2 + 1;
        let first_line_width = content_width.saturating_sub(prefix_width);
        let continuation_width = content_width.saturating_sub(prefix_width);

        let mut is_first_output_line = true;

        let segments = parse_content_segments(message.content());

        for segment in segments {
            match segment {
                ContentSegment::Text(text) => {
                    for content_line in text.lines() {
                        if content_line.is_empty() {
                            if is_first_output_line {
                                lines.push(Line::from(vec![
                                    Span::styled(
                                        format!("[{timestamp}] "),
                                        Style::default().fg(theme.colors.muted),
                                    ),
                                    Span::styled(
                                        format!("<{nick}>"),
                                        Style::default().fg(nick_color),
                                    ),
                                ]));
                                is_first_output_line = false;
                            } else {
                                lines.push(Line::from(""));
                            }
                            continue;
                        }

                        let wrap_width = if is_first_output_line {
                            first_line_width
                        } else {
                            continuation_width
                        };
                        let wrapped = wrap_words(content_line, wrap_width);

                        for wrapped_line in wrapped {
                            let base_style = Style::default().fg(msg_color);
                            let content_spans = if message.role() == MessageRole::Assistant {
                                parse_markdown_line(&wrapped_line, base_style, theme)
                            } else {
                                vec![Span::styled(wrapped_line, base_style)]
                            };

                            if is_first_output_line {
                                let mut line_spans = vec![
                                    Span::styled(
                                        format!("[{timestamp}] "),
                                        Style::default().fg(theme.colors.muted),
                                    ),
                                    Span::styled(
                                        format!("<{nick}>"),
                                        Style::default().fg(nick_color),
                                    ),
                                    Span::raw(" "),
                                ];
                                line_spans.extend(content_spans);
                                lines.push(Line::from(line_spans));
                                is_first_output_line = false;
                            } else {
                                let indent = " ".repeat(prefix_width);
                                let mut line_spans = vec![Span::raw(indent)];
                                line_spans.extend(content_spans);
                                lines.push(Line::from(line_spans));
                            }
                        }
                    }
                }
                ContentSegment::CodeBlock { language, code } => {
                    if is_first_output_line {
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("[{timestamp}] "),
                                Style::default().fg(theme.colors.muted),
                            ),
                            Span::styled(format!("<{nick}>"), Style::default().fg(nick_color)),
                        ]));
                        is_first_output_line = false;
                    }

                    lines.push(Line::from(""));

                    let code_block = CodeBlock::new(&language, &code);
                    let code_lines = code_block.render(theme, continuation_width);

                    let indent = " ".repeat(prefix_width);
                    for code_line in code_lines {
                        let mut line_spans = vec![Span::raw(indent.clone())];
                        line_spans.extend(
                            code_line
                                .spans
                                .into_iter()
                                .map(|s| Span::styled(s.content.to_string(), s.style)),
                        );
                        lines.push(Line::from(line_spans));
                    }

                    lines.push(Line::from(""));
                }
            }
        }
    }

    let visible_height = padded.height as usize;
    let total_lines = lines.len();
    let bottom_start = total_lines.saturating_sub(visible_height);

    app.clamp_scroll(bottom_start);
    let scroll_offset = app.scroll_offset();

    let start = bottom_start.saturating_sub(scroll_offset);

    let visible_lines: Vec<Line> = lines.into_iter().skip(start).take(visible_height).collect();

    let paragraph = Paragraph::new(visible_lines);
    frame.render_widget(paragraph, padded);
}

/// Render the Swarm panel (agent list).
pub(super) fn render_swarm_panel(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let border_color = if is_focused {
        theme.colors.primary
    } else {
        theme.colors.muted
    };

    let block = Block::default()
        .title(" Swarm ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let agents = app.agents();

    if agents.is_empty() {
        let empty_msg = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No agents.",
                Style::default().fg(theme.colors.muted),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Use /swarm to",
                Style::default().fg(theme.colors.muted),
            )),
            Line::from(Span::styled(
                "  manage agents.",
                Style::default().fg(theme.colors.muted),
            )),
        ];
        let paragraph = Paragraph::new(empty_msg);
        frame.render_widget(paragraph, inner);
        return;
    }

    let selected = app.selected_agent();
    let active_id = app.active_agent_id();
    let mut lines: Vec<Line> = Vec::new();

    for (i, agent) in agents.iter().enumerate() {
        let is_selected = i == selected;
        let is_active = agent.id == active_id;

        let prefix = if is_selected && is_focused {
            " > "
        } else if is_active {
            " ● "
        } else {
            "   "
        };

        let line_style = if is_selected && is_focused {
            Style::default()
                .fg(theme.colors.primary)
                .add_modifier(Modifier::BOLD)
        } else if is_active {
            Style::default().fg(theme.colors.secondary)
        } else {
            Style::default().fg(theme.colors.muted)
        };

        let max_name_width = inner.width.saturating_sub(4) as usize;
        let display_name = if display_width(&agent.name) > max_name_width {
            let mut truncated = String::new();
            let mut width = 0;
            for c in agent.name.chars() {
                use unicode_width::UnicodeWidthChar;
                let char_width = c.width().unwrap_or(1);
                if width + char_width >= max_name_width {
                    break;
                }
                truncated.push(c);
                width += char_width;
            }
            format!("{truncated}…")
        } else {
            agent.name.clone()
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{prefix} "), line_style),
            Span::styled(display_name, line_style),
        ]));
    }

    let visible_height = inner.height as usize;
    let scroll = if selected >= visible_height {
        selected.saturating_sub(visible_height / 2)
    } else {
        0
    };

    let visible_lines: Vec<Line> = lines.into_iter().skip(scroll).collect();

    let paragraph = Paragraph::new(visible_lines);
    frame.render_widget(paragraph, inner);
}

/// Render the Record panel.
pub(super) fn render_record_panel(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    use crate::events::RecordStatus;

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

        let (status_text, status_color) = match record.status {
            RecordStatus::Ok => (" ✓ ", theme.colors.success),
            RecordStatus::Error => (" ✗ ", theme.colors.error),
            RecordStatus::Pending => (" ◌ ", theme.colors.pending),
            RecordStatus::None => (" · ", theme.colors.muted),
        };
        let status_style = if is_selected && is_focused {
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(status_color)
        };

        let fixed_width = 29usize;
        let available_info_width = (inner.width as usize).saturating_sub(fixed_width);

        let info_display = if record.info.is_empty() {
            String::new()
        } else if record.info.len() > available_info_width && available_info_width > 3 {
            format!(
                "{}…",
                &record.info[..available_info_width.saturating_sub(1)]
            )
        } else if record.info.len() > available_info_width {
            String::new()
        } else {
            record.info.clone()
        };

        lines.push(Line::from(vec![
            Span::styled(prefix, line_style),
            Span::styled(format!("#{:<3}", record.seq), line_style),
            Span::styled(format!(" {} ", record.timestamp), line_style),
            Span::styled(status_text, status_style),
            Span::styled(format!("{:<8}", record.tx_kind), line_style),
            Span::styled(format!(" {info_display}"), hash_style),
        ]));
    }

    let visible_height = inner.height as usize;

    let scroll = if selected >= visible_height {
        selected.saturating_sub(visible_height / 2)
    } else {
        0
    };

    let visible_lines: Vec<Line> = lines.into_iter().skip(scroll).collect();

    let paragraph = Paragraph::new(visible_lines);
    frame.render_widget(paragraph, inner);
}
