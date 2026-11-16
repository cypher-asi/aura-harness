//! Panel rendering: Chat, Swarm and Record.

use super::header::{self, HeaderInfo, PanelHeader};
use super::markdown::is_inline_code;
use super::text::{display_width, Measure};
use crate::{
    animation::AURA_WIDTH,
    app::{
        tab_bar::TabLayout, AgentsTab, ChatTab, HoverTarget, PanelFocus, ProjectListRow, ProseRow,
        ToggleRow, ToggleTarget,
    },
    App, Theme,
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
    Frame,
};
use std::ops::Range;

/// Render the Chat panel: a CHAT | TERMINAL header bar over the
/// conversation or the shell commands run this session.
pub(super) fn render_chat_panel(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Chat;
    let (bar, area) = header::split(area);
    app.set_chat_tabs_area(bar);
    let hovered = match app.hover() {
        Some(HoverTarget::ChatTab(tab)) => Some(tab),
        _ => None,
    };
    let header = PanelHeader {
        layout: ChatTab::layout(bar.width),
        active: Some(app.chat_tab()),
        hovered,
        title: None,
        info: None,
        focused: is_focused,
    };
    header::render_panel_header(frame, bar, &header, theme);

    let Some(body) = render_body_block(frame, area, is_focused, theme) else {
        return;
    };

    let terminal = app.terminal_pane();
    let padded = if terminal {
        terminal_text_area(body)
    } else {
        centered_text_area(body)
    };
    app.set_chat_width(padded.width);
    app.set_chat_text_area(padded);

    let working_line = if terminal {
        None
    } else {
        working_indicator_line(app, theme)
    };
    if app.pane_is_empty() && working_line.is_none() {
        render_welcome(frame, padded, terminal, theme);
        return;
    }
    let tail: Vec<Line<'static>> = working_line.into_iter().collect();

    let rows = render_chat_lines(frame, padded, app, theme, &tail);
    render_scrollbar(
        frame,
        area,
        padded,
        ScrollView {
            total: rows.total,
            visible: padded.height as usize,
            first: rows.first,
        },
        is_focused,
        theme,
    );
    let below = app.scroll_offset().saturating_sub(rows.padding);
    render_lines_below_hint(frame, area, below, theme);
}

/// Draw a panel's bordered body in `area`, below its header bar, returning
/// the area inside the border when it has any rows.
pub(super) fn render_body_block(
    frame: &mut Frame,
    area: Rect,
    is_focused: bool,
    theme: &Theme,
) -> Option<Rect> {
    let border_color = if is_focused {
        theme.colors.primary
    } else {
        theme.colors.muted
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    (inner.height > 0).then_some(inner)
}

/// How much of a panel's list is on screen.
#[derive(Debug, Clone, Copy)]
pub(super) struct ScrollView {
    /// Rows in the whole list
    pub total: usize,
    /// Rows that fit on screen
    pub visible: usize,
    /// First row on screen
    pub first: usize,
}

/// A scrollbar on `panel`'s right border, beside the `rows` a list is drawn
/// in, when the list is longer than fits. A list that fits keeps the plain
/// border.
pub(super) fn render_scrollbar(
    frame: &mut Frame,
    panel: Rect,
    rows: Rect,
    view: ScrollView,
    is_focused: bool,
    theme: &Theme,
) {
    if view.total <= view.visible || panel.width == 0 || rows.height == 0 {
        return;
    }
    let border = if is_focused {
        theme.colors.primary
    } else {
        theme.colors.muted
    };
    let thumb = if is_focused {
        theme.colors.primary
    } else {
        theme.colors.secondary
    };
    let column = Rect {
        x: panel.x + panel.width - 1,
        y: rows.y,
        width: 1,
        height: rows.height,
    };
    // Positions run from 0 to the last first row, so the thumb reaches the
    // bottom of the track when the list's last row is on screen.
    let mut state = ScrollbarState::new(view.total - view.visible + 1)
        .position(view.first)
        .viewport_content_length(view.visible);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .track_style(Style::default().fg(border))
        .thumb_symbol("┃")
        .thumb_style(Style::default().fg(thumb));
    frame.render_stateful_widget(scrollbar, column, &mut state);
}

/// Draw the rows of the conversation that fall in `padded` at the current
/// scroll position, cloning only those rows out of the line cache, then
/// `tail` (the working indicator) a blank row below them.
fn render_chat_lines(
    frame: &mut Frame,
    padded: Rect,
    app: &mut App,
    theme: &Theme,
    tail: &[Line<'static>],
) -> ChatRows {
    let spinner = app.spinner_char();
    let hovered_activity = match app.hover() {
        Some(HoverTarget::Activity(target)) => Some(target),
        _ => None,
    };
    let measure = Measure::prose(app.prose_measure(), theme);
    let (messages, cache, groups) = app.chat_line_cache();
    let width = padded.width as usize;
    let reflowed = cache.update(messages, (width, measure), theme, spinner, &groups);
    let gap = cache.block_gap();
    let mut toggle_rows = Vec::new();
    let mut message_rows = 0;
    let mut last_prompt_row = None;
    for (i, block) in cache.blocks().enumerate() {
        if i > 0 {
            message_rows += gap;
        }
        if block.user {
            last_prompt_row = Some(message_rows);
        }
        toggle_rows.extend(block.toggles.iter().map(|toggle| ToggleRow {
            line: message_rows + toggle.line,
            ..*toggle
        }));
        message_rows += block.lines.len();
    }
    let tail_gap = usize::from(message_rows > 0 && !tail.is_empty());
    let tail_toggle = app
        .turn_started_ms()
        .filter(|_| !tail.is_empty())
        .map(|started| ToggleRow {
            line: message_rows + tail_gap,
            target: ToggleTarget::TurnRecords(started),
            open: false,
        });
    toggle_rows.extend(tail_toggle);
    let content = message_rows + tail_gap + tail.len();
    let height = padded.height as usize;
    // Blank rows after the end, enough that scrolling to the bottom can
    // bring the latest prompt to the top with the reply filling in below.
    let padding = last_prompt_row.map_or(0, |row| (row + height).saturating_sub(content));
    let total = content + padding;
    let start = app.sync_chat_scroll(total, height, reflowed);
    app.set_chat_layout(toggle_rows, start);

    let (_, cache, _) = app.chat_line_cache();
    let blank = Line::default();
    let mut visible = Vec::with_capacity(height);
    let mut prose = Vec::new();
    let mut line_index = 0usize;
    for (i, block) in cache.blocks().enumerate() {
        if i > 0 {
            for _ in 0..gap {
                push_visible(&mut visible, &mut line_index, start, height, blank.clone());
            }
        }
        let hovered = |offset: usize| {
            block
                .toggles
                .iter()
                .any(|toggle| toggle.line == offset && hovered_activity == Some(toggle.target))
        };
        for (offset, line) in block.lines.iter().enumerate() {
            if line_index >= start && visible.len() < height {
                let mut line = line.clone();
                if hovered(offset) {
                    apply_activity_hover(&mut line, theme);
                }
                if let Some(Some(indent)) = block.prose.get(offset).filter(|_| line.width() > 0) {
                    prose.push((visible.len(), *indent));
                }
                visible.push(line);
            }
            line_index += 1;
        }
    }
    if tail_gap > 0 {
        push_visible(&mut visible, &mut line_index, start, height, blank);
    }
    for (row, line) in tail.iter().enumerate() {
        let mut line = line.clone();
        if row == 0 && tail_toggle.is_some_and(|t| hovered_activity == Some(t.target)) {
            apply_activity_hover(&mut line, theme);
        }
        push_visible(&mut visible, &mut line_index, start, height, line);
    }
    let prose_lines: Vec<(usize, u16, Line<'static>)> = prose
        .into_iter()
        .map(|(row, indent)| (row, indent.min(padded.width), visible[row].clone()))
        .collect();
    frame.render_widget(Paragraph::new(visible), padded);
    let buffer = frame.buffer_mut();
    let prose = prose_lines
        .into_iter()
        .map(|(row, indent, line)| {
            #[allow(clippy::cast_possible_truncation)]
            let row = Rect {
                y: padded.y + row as u16,
                height: 1,
                ..padded
            };
            let (width, code) = if measure.is_grid() {
                prose_columns(&line, row, theme)
            } else {
                pack_prose(buffer, &line, (row, indent), measure, theme)
            };
            ProseRow::new(buffer, (row.x, row.y), (width, indent), code)
        })
        .collect();
    app.set_prose_rows(prose);

    ChatRows {
        total,
        first: start,
        padding,
    }
}

/// How the chat's rows fell in the last frame.
struct ChatRows {
    /// Rows in the whole conversation, blank padding included
    total: usize,
    /// First row on screen
    first: usize,
    /// Blank rows after the end
    padding: usize,
}

/// How many of `area`'s columns a prose `line` covers, and where its
/// inline code falls.
fn prose_columns(line: &Line<'_>, area: Rect, theme: &Theme) -> (u16, Vec<Range<u16>>) {
    let mut code: Vec<Range<u16>> = Vec::new();
    let mut column = area.x;
    let end = area.right();
    for span in &line.spans {
        let width = u16::try_from(span.width()).unwrap_or(u16::MAX);
        let next = column.saturating_add(width).min(end);
        if is_inline_code(span.style, theme) && next > column {
            match code.last_mut() {
                Some(last) if last.end == column => last.end = next,
                _ => code.push(column..next),
            }
        }
        column = next;
    }
    (column - area.x, code)
}

/// Write a prose `line` into `row` with its text after `indent` columns
/// packed into the cells it covers in the front end's face, `measure`
/// wide: each run of one style gets the cells its width rounds to, its
/// characters spread across them. Inline code keeps a cell per column.
/// Returns the columns the line covers and where its inline code falls.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn pack_prose(
    buffer: &mut Buffer,
    line: &Line<'_>,
    (row, indent): (Rect, u16),
    measure: Measure,
    theme: &Theme,
) -> (u16, Vec<Range<u16>>) {
    let mut runs: Vec<(Vec<&str>, Style)> = Vec::new();
    let mut skipped = 0;
    for grapheme in line.styled_graphemes(Style::default()) {
        if skipped < usize::from(indent) {
            skipped += display_width(grapheme.symbol);
            continue;
        }
        match runs.last_mut() {
            Some((symbols, style)) if *style == grapheme.style => symbols.push(grapheme.symbol),
            _ => runs.push((vec![grapheme.symbol], grapheme.style)),
        }
    }

    let mut code: Vec<Range<u16>> = Vec::new();
    let mut cell = indent;
    let mut position = f32::from(indent);
    let mut last = None;
    for (symbols, style) in runs {
        let width = if is_inline_code(style, theme) {
            symbols.iter().map(|s| display_width(s)).sum::<usize>() as f32
        } else {
            measure.width(&symbols.concat(), style)
        };
        let start = cell;
        position += width;
        let end = if is_inline_code(style, theme) {
            start.saturating_add(width as u16)
        } else {
            (position.round() as u16).max(start + 1)
        }
        .min(row.width);
        if end <= start {
            // Past the edge: the rest joins the last cell.
            if let Some(cell) = last.and_then(|x| buffer.cell_mut((x, row.y))) {
                let joined = format!("{}{}", cell.symbol(), symbols.concat());
                cell.set_symbol(&joined);
            }
            continue;
        }
        let cells = usize::from(end - start);
        for i in 0..cells {
            let share = &symbols[i * symbols.len() / cells..(i + 1) * symbols.len() / cells];
            if let Some(slot) = buffer.cell_mut((row.x + start + i as u16, row.y)) {
                slot.reset();
                slot.set_symbol(&share.concat()).set_style(style);
            }
        }
        if is_inline_code(style, theme) {
            code.push(row.x + start..row.x + end);
        }
        last = Some(row.x + end - 1);
        cell = end;
        position = position.max(f32::from(cell) - 0.5);
    }
    for x in cell..row.width {
        if let Some(slot) = buffer.cell_mut((row.x + x, row.y)) {
            slot.reset();
        }
    }
    (cell, code)
}

/// Keep `line` when it falls inside the chat viewport.
fn push_visible<'a>(
    visible: &mut Vec<Line<'a>>,
    line_index: &mut usize,
    start: usize,
    height: usize,
    line: Line<'a>,
) {
    if *line_index >= start && visible.len() < height {
        visible.push(line);
    }
    *line_index += 1;
}

/// Hover style on an activity header. Status colors stay.
fn apply_activity_hover(line: &mut Line<'_>, theme: &Theme) {
    let status = [
        theme.colors.success,
        theme.colors.error,
        theme.colors.warning,
    ];
    for span in &mut line.spans {
        let keep_color = span.style.fg.is_some_and(|color| status.contains(&color));
        if !keep_color {
            span.style = span.style.fg(theme.colors.secondary);
        }
    }
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
const CHAT_MAX_TEXT_WIDTH: u16 = 88;

/// The terminal's text area: all of `area` but a column on each side, so
/// output starts at the top left like a terminal's.
fn terminal_text_area(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        width: area.width.saturating_sub(2),
        ..area
    }
}

/// The chat text column, horizontally centered in `area`.
pub(super) fn centered_text_area(area: Rect) -> Rect {
    let width = area
        .width
        .saturating_sub(CHAT_SIDE_MARGIN * 2)
        .min(CHAT_MAX_TEXT_WIDTH);
    Rect {
        x: area.x + (area.width - width) / 2,
        width,
        ..area
    }
}

/// `AURA  ⠹ thinking… 12s` — shown below the conversation while a turn is
/// in flight and no reply text is streaming. Once the agent has shown
/// something this turn, `AURA` heads that instead, and the indicator sits
/// under it in line with the steps: `  ⠹ thinking… 12s`.
pub(super) fn working_indicator_line<'a>(app: &App, theme: &Theme) -> Option<Line<'a>> {
    if app.has_live_activity() || app.ends_in_live_group() {
        return None;
    }
    let label = app.working_label()?;
    let lead = if app.turn_has_agent_block() {
        Span::raw("  ")
    } else {
        Span::styled(
            "AURA  ",
            Style::default()
                .fg(theme.colors.muted)
                .add_modifier(Modifier::BOLD),
        )
    };
    let mut spans = vec![
        lead,
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
pub(crate) fn format_elapsed(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

fn render_welcome(frame: &mut Frame, area: Rect, terminal: bool, theme: &Theme) {
    let text = if terminal {
        "Enter runs a command. Esc returns to chat."
    } else {
        "Type a message to start chatting, $ for a terminal, or /help for commands."
    };
    let welcome = Line::from(Span::styled(text, Style::default().fg(theme.colors.muted)));
    frame.render_widget(Paragraph::new(welcome).wrap(Wrap { trim: true }), area);
}

/// Render the agents panel: a PROJECTS | AGENTS header bar over either the
/// live agents or the roster of permanent agents.
pub(super) fn render_agents_panel(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let (bar, area) = header::split(area);
    app.set_agent_tabs_area(bar);
    let hovered = match app.hover() {
        Some(HoverTarget::AgentTab(tab)) => Some(tab),
        _ => None,
    };
    let header = PanelHeader {
        layout: AgentsTab::layout(bar.width),
        active: Some(app.agents_tab()),
        hovered,
        title: None,
        info: None,
        focused: is_focused,
    };
    header::render_panel_header(frame, bar, &header, theme);

    let Some(body) = render_body_block(frame, area, is_focused, theme) else {
        return;
    };
    let bottom_row = Rect {
        y: body.y + body.height - 1,
        height: 1,
        ..body
    };
    match app.agents_tab() {
        AgentsTab::Projects => {
            let rows_area = Rect {
                height: body.height - 1,
                ..body
            };
            let visible = rows_area.height as usize;
            app.set_agent_rows_area(rows_area);
            app.set_new_project_area(bottom_row);
            app.sync_agents_scroll(visible);
            let mut lines = build_swarm_lines(app, body, theme);
            let new_project = lines.pop().unwrap_or_default();
            let view = ScrollView {
                total: lines.len(),
                visible,
                first: app.agents_scroll(),
            };
            let lines: Vec<Line> = lines.into_iter().skip(view.first).take(visible).collect();
            frame.render_widget(Paragraph::new(lines), rows_area);
            frame.render_widget(Paragraph::new(new_project), bottom_row);
            render_scrollbar(frame, area, rows_area, view, is_focused, theme);
        }
        AgentsTab::Roster => {
            let rows_area = Rect {
                height: body.height - 1,
                ..body
            };
            let visible = rows_area.height as usize;
            app.set_definition_rows_area(rows_area);
            app.set_new_agent_area(bottom_row);
            app.sync_definitions_scroll(visible);
            let mut lines = build_definition_lines(app, body, theme);
            let new_agent = lines.pop().unwrap_or_default();
            let view = ScrollView {
                total: lines.len(),
                visible,
                first: app.definitions_scroll(),
            };
            let lines: Vec<Line> = lines.into_iter().skip(view.first).take(visible).collect();
            frame.render_widget(Paragraph::new(lines), rows_area);
            frame.render_widget(Paragraph::new(new_agent), bottom_row);
            render_scrollbar(frame, area, rows_area, view, is_focused, theme);
        }
    }
}

/// A project is its own row under a folder icon, with its first agent
/// below it and agents that agent spawned indented further. Each agent
/// shows how long ago it was last active on the right. A ring spins in the
/// folder's column beside agents that are thinking; idle agents leave it
/// blank.
/// Space folds the project and hides every agent under it. Every row is one
/// line, and only the highlighted one has a background: the keyboard cursor
/// while the panel is focused, otherwise the agent shown in the chat.
/// Projects are never highlighted. The
/// last line is "+ New project", which the panel pins to its bottom row:
///
/// ```text
///  ▱  Website
///  ⢎⡱ Agent              1m
///      reviewer          2d
///
///  ▰  Research
///  …
///  +  New project
/// ```
fn build_swarm_lines<'a>(app: &App, inner: Rect, theme: &Theme) -> Vec<Line<'a>> {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let selected = app.selected_agent();
    let active_id = app.active_agent_id();
    let width = inner.width as usize;
    let now_ms = now_ms();
    let is_thinking = |agent: &crate::events::AgentSummary| {
        agent.running || (agent.id == active_id && app.is_processing())
    };
    let project_name = Style::default()
        .fg(theme.colors.muted)
        .add_modifier(Modifier::BOLD);
    let agent_name = Style::default().fg(theme.colors.foreground);
    let mut lines = Vec::new();

    for (row_index, row) in app.project_rows().into_iter().enumerate() {
        let is_selected = match row {
            ProjectListRow::Project { .. } | ProjectListRow::Gap => false,
            _ if is_focused => row_index == selected,
            ProjectListRow::Agent { index } => app.agents()[index].id == active_id,
            ProjectListRow::NewProject => false,
        };
        let is_hovered =
            matches!(app.hover(), Some(HoverTarget::ProjectRow(index)) if index == row_index);
        match row {
            ProjectListRow::Project { root, expanded } => {
                let end = app_project_end(app, root);
                let agents = &app.agents()[root..end];
                let icon = if !expanded && agents.iter().any(is_thinking) {
                    aura_icon(app)
                } else {
                    let folder = if expanded { "▱" } else { "▰" };
                    StatusIcon::new(folder, theme.colors.muted)
                };
                let cell = cell_style(theme, CellState::of(is_selected, is_hovered), project_name);
                lines.push(list_row(
                    width,
                    0,
                    icon,
                    &project_label(&app.agents()[root]),
                    cell,
                    None,
                ));
            }
            ProjectListRow::Agent { index } => {
                let agent = &app.agents()[index];
                let icon = if is_thinking(agent) {
                    aura_icon(app)
                } else {
                    StatusIcon::new("", theme.colors.muted)
                };
                let ago = agent.last_active_ms.map(|ms| format_ago(ms, now_ms));
                let state = CellState::of(is_selected, is_hovered);
                lines.push(list_row(
                    width,
                    agent.depth,
                    icon,
                    &agent.name,
                    cell_style(theme, state, agent_name),
                    ago.as_deref(),
                ));
            }
            ProjectListRow::NewProject => {
                let cell = cell_style(
                    theme,
                    CellState::of(is_selected, is_hovered),
                    Style::default().fg(theme.colors.muted),
                );
                let icon = StatusIcon::new("+", cell.text.fg.unwrap_or(theme.colors.muted));
                lines.push(list_row(width, 0, icon, "New project", cell, None));
            }
            ProjectListRow::Gap => lines.push(Line::default()),
        }
    }
    lines
}

/// What sits in the icon column of a projects-list row.
pub(super) struct StatusIcon {
    glyph: String,
    color: Color,
}

impl StatusIcon {
    pub(super) fn new(glyph: &str, color: Color) -> Self {
        Self {
            glyph: glyph.to_string(),
            color,
        }
    }
}

pub(super) fn aura_icon(app: &App) -> StatusIcon {
    let (glyph, color) = app.aura_frame();
    StatusIcon { glyph, color }
}

/// ` icon name      time `, filled with the row's background (if it has
/// one) from one panel border to the other. The icon column is wide enough
/// for the aura ring, so names line up whatever icon a row shows. `depth`
/// indents the row two columns per level.
pub(super) fn list_row<'a>(
    width: usize,
    depth: usize,
    icon: StatusIcon,
    name: &str,
    cell: CellStyle,
    time: Option<&str>,
) -> Line<'a> {
    let fill = cell
        .bg
        .map_or_else(Style::default, |bg| Style::default().bg(bg));
    let indent = format!(" {}", "  ".repeat(depth));
    let icon_pad = AURA_WIDTH.saturating_sub(display_width(&icon.glyph)) + 1;
    let time = time.map(|time| format!(" {time} ")).unwrap_or_default();
    let name_width = width
        .saturating_sub(
            display_width(&indent) + display_width(&icon.glyph) + icon_pad + display_width(&time),
        )
        .max(1);
    let name = truncate_agent_name(name, name_width);
    let padding = name_width.saturating_sub(display_width(&name));
    let icon_color = cell.icon.unwrap_or(icon.color);
    Line::from(vec![
        Span::styled(indent, fill),
        Span::styled(icon.glyph, fill.fg(icon_color)),
        Span::styled(" ".repeat(icon_pad), fill),
        Span::styled(name, fill.patch(cell.text)),
        Span::styled(" ".repeat(padding), fill),
        Span::styled(time, fill.fg(cell.time)),
    ])
}

/// How a list cell is drawn, from strongest to weakest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CellState {
    /// The keyboard cursor while the panel is focused, otherwise the agent
    /// shown in the chat.
    Selected,
    /// Under the pointer.
    Hovered,
    Normal,
}

impl CellState {
    pub(super) const fn of(selected: bool, hovered: bool) -> Self {
        if selected {
            Self::Selected
        } else if hovered {
            Self::Hovered
        } else {
            Self::Normal
        }
    }
}

/// Colors for one list row. `icon` overrides the icon's own color.
#[derive(Debug, Clone, Copy)]
pub(super) struct CellStyle {
    bg: Option<Color>,
    text: Style,
    icon: Option<Color>,
    time: Color,
}

/// Rows sit on the panel background, turn a subtle gray under the pointer,
/// and turn the selection color with dark text when selected. `text` is the row's name style when nothing else applies.
pub(super) fn cell_style(theme: &Theme, state: CellState, text: Style) -> CellStyle {
    let colors = &theme.colors;
    let plain = |text: Style| CellStyle {
        bg: None,
        text,
        icon: None,
        time: colors.muted,
    };
    match state {
        CellState::Selected => CellStyle {
            bg: Some(colors.selection),
            text: text.fg(colors.on_selection),
            icon: Some(colors.on_selection),
            time: colors.on_selection,
        },
        CellState::Hovered => CellStyle {
            bg: Some(colors.surface_hover),
            ..plain(text)
        },
        CellState::Normal => plain(text),
    }
}

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// `now`, `5m`, `3h`, `2d`, `6w`: how long before `now_ms` `then_ms` was.
pub(super) fn format_ago(then_ms: u64, now_ms: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        s if s < MINUTE => "now".to_string(),
        s if s < HOUR => format!("{}m", s / MINUTE),
        s if s < DAY => format!("{}h", s / HOUR),
        s if s < WEEK => format!("{}d", s / DAY),
        s => format!("{}w", s / WEEK),
    }
}

/// First index past the agents belonging to the project rooted at `root`.
fn app_project_end(app: &App, root: usize) -> usize {
    app.agents()
        .iter()
        .skip(root + 1)
        .position(|agent| agent.depth == 0)
        .map_or(app.agents().len(), |offset| root + 1 + offset)
}

fn project_label(agent: &crate::events::AgentSummary) -> String {
    agent
        .project
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map_or_else(|| agent.name.clone(), ToString::to_string)
}

/// Secondary color for a clickable item under the pointer.
pub(super) fn hover_style(theme: &Theme) -> Style {
    Style::default().fg(theme.colors.secondary)
}

/// Roster rows: a gray `@` and the name for each permanent agent, then
/// "+  New agent", which the panel pins to its bottom row. Markers and
/// names share columns with the projects list. A selected row still paints
/// the `@` in the selection text color.
fn build_definition_lines<'a>(app: &App, inner: Rect, theme: &Theme) -> Vec<Line<'a>> {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let selected = app.selected_definition();
    let width = inner.width as usize;
    let names = app
        .definitions()
        .iter()
        .map(|definition| {
            (
                "@",
                definition.name.as_str(),
                theme.colors.secondary,
                theme.colors.muted,
            )
        })
        .chain(std::iter::once((
            "+",
            "New agent",
            theme.colors.muted,
            theme.colors.muted,
        )));

    names
        .enumerate()
        .map(|(i, (marker, name, text, icon_color))| {
            let is_selected = i == selected && is_focused;
            let is_hovered =
                matches!(app.hover(), Some(HoverTarget::DefinitionRow(row)) if row == i);
            let state = CellState::of(is_selected, is_hovered);
            let cell = cell_style(theme, state, Style::default().fg(text));
            let icon = StatusIcon::new(marker, icon_color);
            list_row(width, 0, icon, name, cell, None)
        })
        .collect()
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

/// Render the Kernel panel, which lists the kernel's records under a
/// header bar with no tabs.
pub(super) fn render_record_panel(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Records;
    let (bar, area) = header::split(area);
    let header = PanelHeader {
        layout: TabLayout::untabbed(bar.width),
        active: None,
        hovered: None,
        title: Some("KERNEL"),
        info: Some(HeaderInfo::count(
            app.records().len(),
            "RECORDS",
            theme.colors.muted,
        )),
        focused: is_focused,
    };
    header::render_panel_header(frame, bar, &header, theme);

    let Some(inner) = render_body_block(frame, area, is_focused, theme) else {
        return;
    };

    let records = app.records();
    if records.is_empty() {
        render_empty_records(frame, inner, theme);
        return;
    }

    let visible = inner.height as usize;
    let scroll = app.sync_records_scroll(visible);
    app.set_record_rows_area(inner);
    let lines = build_record_lines(app, inner, scroll, visible, theme);
    frame.render_widget(Paragraph::new(lines), inner);
    let view = ScrollView {
        total: app.records().len(),
        visible,
        first: scroll,
    };
    render_scrollbar(frame, area, inner, view, is_focused, theme);
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
    let hovered = match app.hover() {
        Some(HoverTarget::RecordRow(index)) => Some(index),
        _ => None,
    };
    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    for (i, record) in app.records().iter().enumerate().skip(first).take(rows) {
        let is_selected = i == selected;
        let prefix = " ";

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

        let mut fixed_spans = vec![
            Span::styled(prefix, line_style),
            Span::styled(format!("#{:<3}", record.seq), line_style),
            Span::styled(status_text, status_style),
            Span::styled(fit_column(&record.tx_kind, KIND_WIDTH), kind_style),
        ];
        // Records without columns show their one-line summary after the kind.
        let detail = if record.what.is_empty() {
            &record.info
        } else {
            let is_failure = matches!(record.what.as_str(), "error" | "failed");
            let what_style = if is_selected && is_focused {
                line_style
            } else if is_failure {
                Style::default().fg(theme.colors.error)
            } else {
                Style::default().fg(theme.colors.foreground)
            };
            fixed_spans.push(Span::styled(
                format!(" {}", fit_column(&record.what, WHAT_WIDTH)),
                what_style,
            ));
            fixed_spans.push(Span::styled(
                format!(" {}", fit_column(&record.group, GROUP_WIDTH)),
                Style::default().fg(theme.colors.muted),
            ));
            &record.detail
        };
        let used: usize = fixed_spans.iter().map(|s| display_width(&s.content)).sum();
        // One column for the separator space before the detail text.
        let available = width.saturating_sub(used + 1);
        let info_display = truncate_record_info(detail, available);

        let mut spans = fixed_spans;
        if !info_display.is_empty() {
            spans.push(Span::styled(format!(" {info_display}"), hash_style));
        }
        if hovered == Some(i) && !(is_selected && is_focused) {
            let bg = theme.colors.surface_hover;
            let filled: usize = spans.iter().map(|s| display_width(&s.content)).sum();
            spans.push(Span::raw(" ".repeat(width.saturating_sub(filled))));
            for span in &mut spans {
                span.style = span.style.bg(bg);
            }
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// Widths of the Record panel's kind, what and group columns.
const KIND_WIDTH: usize = 8;
const WHAT_WIDTH: usize = 11;
const GROUP_WIDTH: usize = 7;

/// `text` upper-cased and padded or cut to exactly `width` columns.
fn fit_column(text: &str, width: usize) -> String {
    let fitted = truncate_record_info(&text.to_uppercase(), width);
    let pad = width.saturating_sub(display_width(&fitted));
    format!("{fitted}{}", " ".repeat(pad))
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
