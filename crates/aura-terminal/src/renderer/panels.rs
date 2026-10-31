//! Panel rendering: Chat, Swarm, Record, and Thinking section.

use super::markdown::{parse_content_segments, ContentSegment};
use super::segments::{
    message_style, nick_label, render_code_segment, render_text_segment, NICK_WIDTH,
};
use super::text::{display_width, wrap_words};
use crate::{app::PanelFocus, App, Theme};
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
        .title(" CHAT ")
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

    let working_line = working_indicator_line(app, theme);
    if app.messages().is_empty() && working_line.is_none() {
        render_welcome(frame, padded, theme);
        return;
    }

    let mut lines = build_chat_lines(app, padded.width as usize, theme);
    lines.extend(working_line);
    render_scrolled_lines(frame, padded, app, lines);
}

/// `           <AURA> ⠹ thinking… 12s` — shown below the conversation while a
/// turn is in flight and no reply text is streaming.
pub(super) fn working_indicator_line<'a>(app: &App, theme: &Theme) -> Option<Line<'a>> {
    let label = app.working_label()?;
    let mut spans = vec![
        // Blank where the timestamp column sits, so the nick lines up.
        Span::raw(" ".repeat(11)),
        Span::styled(nick_label("AURA"), Style::default().fg(theme.colors.muted)),
        Span::raw(" "),
        Span::styled(
            format!("{} ", app.spinner_char()),
            Style::default().fg(theme.colors.secondary),
        ),
        Span::styled(
            format!("{label}…"),
            Style::default()
                .fg(theme.colors.muted)
                .add_modifier(Modifier::ITALIC),
        ),
    ];
    if let Some(elapsed) = app.processing_elapsed().filter(|e| e.as_secs() > 0) {
        spans.push(Span::styled(
            format!(" {}", format_elapsed(elapsed)),
            Style::default()
                .fg(theme.colors.muted)
                .add_modifier(Modifier::DIM),
        ));
    }
    Some(Line::from(spans))
}

/// `7s`, `2m 05s`.
pub(super) fn format_elapsed(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
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
        let prefix_width = 11 + NICK_WIDTH + 2 + 1;
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

/// Hint shown at the bottom of the Agents panel.
const SWARM_HINT: &str = "Alt+↑↓ switch  ^N new";

/// Render the Swarm panel (agent list followed by a "+ New agent" row).
pub(super) fn render_swarm_panel(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let border_color = if is_focused {
        theme.colors.primary
    } else {
        theme.colors.muted
    };

    let block = Block::default()
        .title(" AGENTS ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let show_hint = inner.height >= 4;
    let rows_area = Rect {
        height: inner.height.saturating_sub(u16::from(show_hint)),
        ..inner
    };
    app.set_agent_rows_area(rows_area);
    app.sync_agents_scroll(rows_area.height as usize);

    let lines: Vec<Line> = build_swarm_lines(app, inner, theme)
        .into_iter()
        .skip(app.agents_scroll())
        .take(rows_area.height as usize)
        .collect();
    frame.render_widget(Paragraph::new(lines), rows_area);

    if show_hint {
        let hint_area = Rect {
            y: inner.y + inner.height - 1,
            height: 1,
            ..inner
        };
        let hint = truncate_agent_name(SWARM_HINT, (inner.width as usize).saturating_sub(1));
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {hint}"),
                Style::default().fg(theme.colors.muted),
            ))),
            hint_area,
        );
    }
}

fn build_swarm_lines<'a>(app: &App, inner: Rect, theme: &Theme) -> Vec<Line<'a>> {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let selected = app.selected_agent();
    let active_id = app.active_agent_id();
    let mut lines: Vec<Line> = Vec::new();
    let number_width = app.agents().len().max(1).to_string().len();

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

        let index = format!("{:>number_width$}. ", i + 1);
        let count = format!(" {}", agent.record_count);
        let name_width = (inner.width as usize)
            .saturating_sub(display_width(prefix) + index.len() + count.len())
            .max(1);
        let display_name = truncate_agent_name(&agent.name, name_width);
        let padding = name_width.saturating_sub(display_width(&display_name));
        lines.push(Line::from(vec![
            Span::styled(prefix, line_style),
            Span::styled(index, line_style),
            Span::styled(display_name, line_style),
            Span::raw(" ".repeat(padding)),
            Span::styled(count, Style::default().fg(theme.colors.muted)),
        ]));
    }

    let new_selected = selected == app.agents().len() && is_focused;
    let new_style = if new_selected {
        Style::default()
            .fg(theme.colors.primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.colors.muted)
    };
    let prefix = if new_selected { " > " } else { "   " };
    lines.push(Line::from(Span::styled(
        format!("{prefix}{:>number_width$}  New agent", "+"),
        new_style,
    )));
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
        .title(" RECORD ")
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

        let kind_style = if is_selected && is_focused {
            line_style
        } else {
            Style::default().fg(record_kind_color(&record.tx_kind, theme))
        };

        let fixed_spans = vec![
            Span::styled(prefix, line_style),
            Span::styled(format!("#{:<3}", record.seq), line_style),
            Span::styled(format!(" {} ", record.timestamp), line_style),
            Span::styled(status_text, status_style),
            Span::styled(format!("{:<8}", record.tx_kind), kind_style),
        ];
        let used: usize = fixed_spans.iter().map(|s| display_width(&s.content)).sum();
        // One column for the separator space before the info text.
        let available = (inner.width as usize).saturating_sub(used + 1);
        let info_display = truncate_record_info(&record.info, available);

        let mut spans = fixed_spans;
        if !info_display.is_empty() {
            spans.push(Span::styled(format!(" {info_display}"), hash_style));
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// Color for the kind column, grouped so tool activity stands out from
/// conversation and bookkeeping rows.
fn record_kind_color(kind: &str, theme: &Theme) -> ratatui::style::Color {
    match kind {
        "ToolCall" | "ToolRes" | "Action" | "Complete" => theme.colors.secondary,
        "Prompt" | "Reply" | "Trigger" => theme.colors.primary,
        "LLM" | "Spawn" => theme.colors.pending,
        _ => theme.colors.muted,
    }
}

/// Fit `info` into `max_width` display columns on a single line.
///
/// Control characters (including newlines) are folded into spaces so a
/// row can never spill onto a second line, and truncation is measured in
/// display width so wide (CJK/emoji) characters never split mid-glyph.
pub(super) fn truncate_record_info(info: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthChar;

    let mut single_line = String::with_capacity(info.len());
    let mut last_was_space = true;
    for c in info.chars() {
        if c.is_whitespace() || c.is_control() {
            if !last_was_space {
                single_line.push(' ');
                last_was_space = true;
            }
        } else {
            single_line.push(c);
            last_was_space = false;
        }
    }
    let single_line = single_line.trim_end();

    if max_width == 0 || single_line.is_empty() {
        return String::new();
    }
    if display_width(single_line) <= max_width {
        return single_line.to_string();
    }

    let budget = max_width - 1;
    let mut truncated = String::new();
    let mut width = 0;
    for c in single_line.chars() {
        let char_width = c.width().unwrap_or(0);
        if width + char_width > budget {
            break;
        }
        truncated.push(c);
        width += char_width;
    }
    format!("{}…", truncated.trim_end())
}
