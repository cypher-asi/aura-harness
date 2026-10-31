//! Panel rendering: Chat, Swarm, Record, and Thinking section.

use super::text::{display_width, wrap_words};
use crate::{
    app::{AgentsTab, PanelFocus},
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
    for line in thinking_content.lines().rev() {
        if wrapped_lines.len() >= 3 {
            break;
        }
        if line.is_empty() {
            wrapped_lines.insert(0, String::new());
        } else {
            wrapped_lines.splice(0..0, wrap_words(line, max_width));
        }
    }
    let start = wrapped_lines.len().saturating_sub(3);
    wrapped_lines.split_off(start)
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

    let padded = centered_text_area(messages_area);
    app.set_chat_width(padded.width);
    app.set_chat_text_area(padded);

    let working_line = working_indicator_line(app, theme);
    if app.messages().is_empty() && working_line.is_none() {
        render_welcome(frame, padded, theme);
        return;
    }

    render_chat_lines(frame, padded, app, theme, working_line.as_ref());
    render_lines_below_hint(frame, area, app.scroll_offset(), theme);
}

/// Draw the rows of the conversation that fall in `padded` at the current
/// scroll position, cloning only those rows out of the line cache.
fn render_chat_lines(
    frame: &mut Frame,
    padded: Rect,
    app: &mut App,
    theme: &Theme,
    working_line: Option<&Line<'static>>,
) {
    let (messages, cache) = app.chat_line_cache();
    let reflowed = cache.update(messages, padded.width as usize, theme);
    let separators = messages.len().saturating_sub(1);
    let message_rows = messages.iter().map(|m| cache.lines(m).len()).sum::<usize>() + separators;
    let working_rows = match working_line {
        Some(_) if message_rows > 0 => 2,
        Some(_) => 1,
        None => 0,
    };
    let start = app.sync_chat_scroll(
        message_rows + working_rows,
        padded.height as usize,
        reflowed,
    );

    let (messages, cache) = app.chat_line_cache();
    let blank = Line::default();
    let working = working_line
        .into_iter()
        .flat_map(|line| [(message_rows > 0).then_some(&blank), Some(line)])
        .flatten();
    let visible: Vec<Line> = messages
        .iter()
        .enumerate()
        .flat_map(|(i, message)| {
            (i > 0)
                .then_some(&blank)
                .into_iter()
                .chain(cache.lines(message))
        })
        .chain(working)
        .skip(start)
        .take(padded.height as usize)
        .cloned()
        .collect();
    frame.render_widget(Paragraph::new(visible), padded);
}

/// `↓ 42 lines below` on the chat panel's bottom border while scrolled up.
fn render_lines_below_hint(frame: &mut Frame, panel: Rect, below: usize, theme: &Theme) {
    if below == 0 || panel.width < 4 || panel.height < 2 {
        return;
    }
    let label = format!(
        " ↓ {below} line{} below ",
        if below == 1 { "" } else { "s" }
    );
    let border_row = Rect {
        x: panel.x + 1,
        y: panel.y + panel.height - 1,
        width: panel.width - 2,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            label,
            Style::default().fg(theme.colors.primary),
        )))
        .alignment(ratatui::layout::Alignment::Right),
        border_row,
    );
}

/// Narrowest gap kept between the chat text and each side of the panel.
const CHAT_SIDE_MARGIN: u16 = 4;

/// Widest the chat text runs; wider panels grow the side margins instead.
const CHAT_MAX_TEXT_WIDTH: u16 = 100;

/// The chat text column, horizontally centered in `area` below a blank row.
pub(super) fn centered_text_area(area: Rect) -> Rect {
    let width = area
        .width
        .saturating_sub(CHAT_SIDE_MARGIN * 2)
        .min(CHAT_MAX_TEXT_WIDTH);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y.saturating_add(1),
        width,
        height: area.height.saturating_sub(1),
    }
}

/// `AURA  ⠹ thinking… 12s` — shown below the conversation while a turn is
/// in flight and no reply text is streaming.
pub(super) fn working_indicator_line<'a>(app: &App, theme: &Theme) -> Option<Line<'a>> {
    let label = app.working_label()?;
    let mut spans = vec![
        Span::styled(
            "AURA  ",
            Style::default()
                .fg(theme.colors.muted)
                .add_modifier(Modifier::BOLD),
        ),
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

/// Hints shown at the bottom of the agents panel, per tab.
const PROJECTS_HINT: &str = "Alt+↑↓ switch  ^N new";
const ROSTER_HINT: &str = "⏎ spawn e edit d del";
/// Shown at the right of the tab bar when there is room.
const TABS_HINT: &str = "←→";

/// Render the agents panel: a Projects | Agents tab bar over either the
/// live agents or the roster of permanent agents.
pub(super) fn render_agents_panel(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
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
    if inner.height == 0 {
        return;
    }

    let header_height = if inner.height >= 6 { 2 } else { 1 };
    app.set_agent_tabs_area(Rect { height: 1, ..inner });
    frame.render_widget(
        Paragraph::new(build_tab_lines(app, inner.width as usize, theme)),
        Rect {
            height: header_height,
            ..inner
        },
    );

    let body = Rect {
        y: inner.y + header_height,
        height: inner.height - header_height,
        ..inner
    };
    let show_hint = body.height >= 3;
    let rows_area = Rect {
        height: body.height.saturating_sub(u16::from(show_hint)),
        ..body
    };
    let visible = rows_area.height as usize;
    let (lines, hint) = match app.agents_tab() {
        AgentsTab::Projects => {
            app.set_agent_rows_area(rows_area);
            app.sync_agents_scroll(visible);
            let lines: Vec<Line> = build_swarm_lines(app, inner, theme)
                .into_iter()
                .skip(app.agents_scroll())
                .take(visible)
                .collect();
            (lines, PROJECTS_HINT)
        }
        AgentsTab::Roster => {
            app.set_definition_rows_area(rows_area);
            app.sync_definitions_scroll(visible);
            let lines: Vec<Line> = build_definition_lines(app, inner, theme)
                .into_iter()
                .skip(app.definitions_scroll())
                .take(visible)
                .collect();
            (lines, ROSTER_HINT)
        }
    };
    frame.render_widget(Paragraph::new(lines), rows_area);

    if show_hint {
        let hint_area = Rect {
            y: body.y + body.height - 1,
            height: 1,
            ..body
        };
        let hint = truncate_agent_name(hint, (inner.width as usize).saturating_sub(1));
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {hint}"),
                Style::default().fg(theme.colors.muted),
            ))),
            hint_area,
        );
    }
}

/// The tab labels, and below them a rule that is heavy under the active tab.
fn build_tab_lines<'a>(app: &App, width: usize, theme: &Theme) -> Vec<Line<'a>> {
    let active = app.agents_tab();
    let muted = Style::default().fg(theme.colors.muted);
    let active_color = if app.focus() == PanelFocus::Swarm {
        theme.colors.primary
    } else {
        theme.colors.secondary
    };
    let mut labels = vec![Span::raw(" ")];
    let mut rule = vec![Span::styled("─", muted)];
    let mut used = 1;
    for (tab, _, tab_width) in AgentsTab::layout() {
        let tab_width = usize::from(tab_width);
        if tab == active {
            let style = Style::default()
                .fg(active_color)
                .add_modifier(Modifier::BOLD);
            labels.push(Span::styled(format!(" {} ", tab.label()), style));
            rule.push(Span::styled(
                "━".repeat(tab_width),
                Style::default().fg(active_color),
            ));
        } else {
            labels.push(Span::styled(format!(" {} ", tab.label()), muted));
            rule.push(Span::styled("─".repeat(tab_width), muted));
        }
        used += tab_width;
    }
    let rest = width.saturating_sub(used);
    if rest > display_width(TABS_HINT) {
        labels.push(Span::styled(format!("{TABS_HINT:>rest$}"), muted));
    }
    rule.push(Span::styled("─".repeat(rest), muted));
    vec![Line::from(labels), Line::from(rule)]
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
            " > ".to_string()
        } else if agent.running {
            format!(" {} ", app.spinner_char())
        } else if is_active {
            " ● ".to_string()
        } else {
            "   ".to_string()
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
        let indent = "  ".repeat(agent.depth);
        let name_width = (inner.width as usize)
            .saturating_sub(display_width(&prefix) + index.len() + indent.len() + count.len())
            .max(1);
        let display_name = truncate_agent_name(&agent.name, name_width);
        let padding = name_width.saturating_sub(display_width(&display_name));
        lines.push(Line::from(vec![
            Span::styled(prefix, line_style),
            Span::styled(index, line_style),
            Span::raw(indent),
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
        format!("{prefix}{:>number_width$}  New project", "+"),
        new_style,
    )));
    lines
}

/// Roster rows: "•  name" for each permanent agent, then "+  New agent",
/// with the markers and the names in the same columns.
fn build_definition_lines<'a>(app: &App, inner: Rect, theme: &Theme) -> Vec<Line<'a>> {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let selected = app.selected_definition();
    let width = inner.width as usize;
    let highlighted = Style::default()
        .fg(theme.colors.primary)
        .add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line> = Vec::with_capacity(app.definitions().len() + 1);

    for (i, definition) in app.definitions().iter().enumerate() {
        let is_selected = i == selected && is_focused;
        let (prefix, name_style) = if is_selected {
            (" > ", highlighted)
        } else {
            ("   ", Style::default().fg(theme.colors.secondary))
        };
        lines.push(Line::from(Span::styled(
            truncate_agent_name(&format!("{prefix}•  {}", definition.name), width.max(1)),
            name_style,
        )));
    }

    let new_selected = selected == app.definitions().len() && is_focused;
    let (prefix, style) = if new_selected {
        (" > ", highlighted)
    } else {
        ("   ", Style::default().fg(theme.colors.muted))
    };
    lines.push(Line::from(Span::styled(
        truncate_agent_name(&format!("{prefix}+  New agent"), width.max(1)),
        style,
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
pub(super) fn render_record_panel(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
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

    let visible = inner.height as usize;
    let scroll = app.sync_records_scroll(visible);
    let lines = build_record_lines(app, inner, scroll, visible, theme);
    frame.render_widget(Paragraph::new(lines), inner);
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

/// Lines for the `rows` records starting at index `first`.
fn build_record_lines<'a>(
    app: &App,
    inner: Rect,
    first: usize,
    rows: usize,
    theme: &Theme,
) -> Vec<Line<'a>> {
    use crate::events::RecordStatus;

    let is_focused = app.focus() == PanelFocus::Records;
    let selected = app.selected_record();
    let mut lines: Vec<Line> = Vec::new();

    for (i, record) in app.records().iter().enumerate().skip(first).take(rows) {
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
