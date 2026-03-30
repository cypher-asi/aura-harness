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
    let divider_width = area.width as usize;

    let label = if app.is_thinking() {
        format!(" Thinker  {}  ", app.spinner_char())
    } else {
        " Thinker ".to_string()
    };
    let right_dashes = divider_width.saturating_sub(label.chars().count());

    let divider_spans = vec![
        Span::styled(label, Style::default().fg(theme.colors.muted)),
        Span::styled(
            "─".repeat(right_dashes),
            Style::default().fg(theme.colors.muted),
        ),
    ];

    let content_lines = build_thinking_lines(app, area);

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

fn build_thinking_lines(app: &App, area: Rect) -> Vec<String> {
    let thinking_content = app.thinking_content();
    if thinking_content.is_empty() {
        if app.is_thinking() {
            return vec!["...".to_string()];
        }
        return vec![];
    }
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
}

/// Render the Chat panel.
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

    let (messages_area, thinking_area) = split_chat_area(inner, app);
    if let Some(think_area) = thinking_area {
        render_thinking_section(frame, think_area, app, theme);
    }

    let padded = Rect {
        x: messages_area.x.saturating_add(2),
        y: messages_area.y.saturating_add(1),
        width: messages_area.width.saturating_sub(4),
        height: messages_area.height.saturating_sub(1),
    };

    if app.messages().is_empty() {
        render_welcome(frame, padded, theme);
        return;
    }

    let lines = build_chat_lines(app, padded.width as usize, theme);
    render_scrolled_lines(frame, padded, app, lines);
}

fn split_chat_area(inner: Rect, app: &App) -> (Rect, Option<Rect>) {
    let thinking_section_height = 4u16;
    let show_thinking_section = app.is_thinking() || !app.thinking_content().is_empty();

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
    }
}

fn render_welcome(frame: &mut Frame, area: Rect, theme: &Theme) {
    let welcome = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Type a message to start chatting, or /help for commands.",
            Style::default().fg(theme.colors.muted),
        )),
    ];
    frame.render_widget(Paragraph::new(welcome), area);
}

fn build_chat_lines<'a>(app: &App, content_width: usize, theme: &Theme) -> Vec<Line<'a>> {
    let mut lines: Vec<Line> = Vec::new();

    for message in app.messages() {
        let (nick, nick_color, msg_color) = message_style(message.role(), theme);
        let timestamp = message.timestamp_local();
        let prefix_width = 11 + nick.len() + 2 + 1;
        let first_line_width = content_width.saturating_sub(prefix_width);
        let continuation_width = content_width.saturating_sub(prefix_width);

        let mut is_first_output_line = true;
        let segments = parse_content_segments(message.content());

        for segment in segments {
            match segment {
                ContentSegment::Text(text) => render_text_segment(
                    &mut lines,
                    &text,
                    &mut is_first_output_line,
                    &timestamp,
                    nick,
                    nick_color,
                    msg_color,
                    prefix_width,
                    first_line_width,
                    continuation_width,
                    message.role(),
                    theme,
                ),
                ContentSegment::CodeBlock { language, code } => render_code_segment(
                    &mut lines,
                    &language,
                    &code,
                    &mut is_first_output_line,
                    &timestamp,
                    nick,
                    nick_color,
                    prefix_width,
                    continuation_width,
                    theme,
                ),
            }
        }
    }
    lines
}

const fn message_style<'a>(
    role: MessageRole,
    theme: &Theme,
) -> (&'a str, ratatui::style::Color, ratatui::style::Color) {
    match role {
        MessageRole::User => ("YOU", theme.colors.foreground, theme.colors.primary),
        MessageRole::Assistant => ("AURA", theme.colors.foreground, theme.colors.muted),
        MessageRole::System => ("*", theme.colors.muted, theme.colors.muted),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_text_segment(
    lines: &mut Vec<Line<'_>>,
    text: &str,
    is_first: &mut bool,
    timestamp: &str,
    nick: &str,
    nick_color: ratatui::style::Color,
    msg_color: ratatui::style::Color,
    prefix_width: usize,
    first_line_width: usize,
    continuation_width: usize,
    role: MessageRole,
    theme: &Theme,
) {
    for content_line in text.lines() {
        if content_line.is_empty() {
            if *is_first {
                lines.push(line_with_prefix(timestamp, nick, nick_color, &[]));
                *is_first = false;
            } else {
                lines.push(Line::from(""));
            }
            continue;
        }
        let wrap_width = if *is_first {
            first_line_width
        } else {
            continuation_width
        };
        let wrapped = wrap_words(content_line, wrap_width);
        for wrapped_line in wrapped {
            let base_style = Style::default().fg(msg_color);
            let content_spans = if role == MessageRole::Assistant {
                parse_markdown_line(&wrapped_line, base_style, theme)
            } else {
                vec![Span::styled(wrapped_line, base_style)]
            };
            if *is_first {
                let mut spans = vec![
                    Span::styled(
                        format!("[{timestamp}] "),
                        Style::default().fg(theme.colors.muted),
                    ),
                    Span::styled(format!("<{nick}>"), Style::default().fg(nick_color)),
                    Span::raw(" "),
                ];
                spans.extend(content_spans);
                lines.push(Line::from(spans));
                *is_first = false;
            } else {
                let indent = " ".repeat(prefix_width);
                let mut spans = vec![Span::raw(indent)];
                spans.extend(content_spans);
                lines.push(Line::from(spans));
            }
        }
    }
}

fn line_with_prefix<'a>(
    timestamp: &str,
    nick: &str,
    nick_color: ratatui::style::Color,
    extra: &[Span<'a>],
) -> Line<'a> {
    let muted = ratatui::style::Color::Rgb(136, 136, 136);
    let mut spans = vec![
        Span::styled(format!("[{timestamp}] "), Style::default().fg(muted)),
        Span::styled(format!("<{nick}>"), Style::default().fg(nick_color)),
    ];
    spans.extend_from_slice(extra);
    Line::from(spans)
}

#[allow(clippy::too_many_arguments)]
fn render_code_segment(
    lines: &mut Vec<Line<'_>>,
    language: &str,
    code: &str,
    is_first: &mut bool,
    timestamp: &str,
    nick: &str,
    nick_color: ratatui::style::Color,
    prefix_width: usize,
    continuation_width: usize,
    theme: &Theme,
) {
    if *is_first {
        lines.push(line_with_prefix(timestamp, nick, nick_color, &[]));
        *is_first = false;
    }
    lines.push(Line::from(""));
    let code_block = CodeBlock::new(language, code);
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

fn render_scrolled_lines(frame: &mut Frame, padded: Rect, app: &mut App, lines: Vec<Line<'_>>) {
    let visible_height = padded.height as usize;
    let total_lines = lines.len();
    let bottom_start = total_lines.saturating_sub(visible_height);

    app.clamp_scroll(bottom_start);
    let scroll_offset = app.scroll_offset();
    let start = bottom_start.saturating_sub(scroll_offset);

    let visible_lines: Vec<Line> = lines.into_iter().skip(start).take(visible_height).collect();
    frame.render_widget(Paragraph::new(visible_lines), padded);
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
        render_empty_swarm(frame, inner, theme);
        return;
    }

    let lines = build_swarm_lines(app, inner, theme);

    let selected = app.selected_agent();
    let visible_height = inner.height as usize;
    let scroll = if selected >= visible_height {
        selected.saturating_sub(visible_height / 2)
    } else {
        0
    };
    let visible_lines: Vec<Line> = lines.into_iter().skip(scroll).collect();
    frame.render_widget(Paragraph::new(visible_lines), inner);
}

fn render_empty_swarm(frame: &mut Frame, inner: Rect, theme: &Theme) {
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
    frame.render_widget(Paragraph::new(empty_msg), inner);
}

fn build_swarm_lines<'a>(app: &App, inner: Rect, theme: &Theme) -> Vec<Line<'a>> {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let selected = app.selected_agent();
    let active_id = app.active_agent_id();
    let mut lines: Vec<Line> = Vec::new();

    for (i, agent) in app.agents().iter().enumerate() {
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

        let display_name = truncate_agent_name(&agent.name, inner.width.saturating_sub(4) as usize);
        lines.push(Line::from(vec![
            Span::styled(format!("{prefix} "), line_style),
            Span::styled(display_name, line_style),
        ]));
    }
    lines
}

fn truncate_agent_name(name: &str, max_width: usize) -> String {
    if display_width(name) <= max_width {
        return name.to_string();
    }
    let mut truncated = String::new();
    let mut width = 0;
    for c in name.chars() {
        use unicode_width::UnicodeWidthChar;
        let char_width = c.width().unwrap_or(1);
        if width + char_width >= max_width {
            break;
        }
        truncated.push(c);
        width += char_width;
    }
    format!("{truncated}…")
}

/// Render the Record panel.
pub(super) fn render_record_panel(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
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
        render_empty_records(frame, inner, theme);
        return;
    }

    let lines = build_record_lines(app, inner, theme);

    let selected = app.selected_record();
    let visible_height = inner.height as usize;
    let scroll = if selected >= visible_height {
        selected.saturating_sub(visible_height / 2)
    } else {
        0
    };
    let visible_lines: Vec<Line> = lines.into_iter().skip(scroll).collect();
    frame.render_widget(Paragraph::new(visible_lines), inner);
}

fn render_empty_records(frame: &mut Frame, inner: Rect, theme: &Theme) {
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
    frame.render_widget(Paragraph::new(empty_msg), inner);
}

fn build_record_lines<'a>(app: &App, inner: Rect, theme: &Theme) -> Vec<Line<'a>> {
    use crate::events::RecordStatus;

    let is_focused = app.focus() == PanelFocus::Records;
    let selected = app.selected_record();
    let mut lines: Vec<Line> = Vec::new();

    for (i, record) in app.records().iter().enumerate() {
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

        let info_display = truncate_record_info(&record.info, inner.width as usize);

        lines.push(Line::from(vec![
            Span::styled(prefix, line_style),
            Span::styled(format!("#{:<3}", record.seq), line_style),
            Span::styled(format!(" {} ", record.timestamp), line_style),
            Span::styled(status_text, status_style),
            Span::styled(format!("{:<8}", record.tx_kind), line_style),
            Span::styled(format!(" {info_display}"), hash_style),
        ]));
    }
    lines
}

fn truncate_record_info(info: &str, panel_width: usize) -> String {
    let fixed_width = 29usize;
    let available = panel_width.saturating_sub(fixed_width);
    if info.is_empty() {
        String::new()
    } else if info.len() > available && available > 3 {
        format!("{}…", &info[..available.saturating_sub(1)])
    } else if info.len() > available {
        String::new()
    } else {
        info.to_string()
    }
}
