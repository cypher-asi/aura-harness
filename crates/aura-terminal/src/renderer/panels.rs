//! Panel rendering: Chat, Swarm and Record.

use super::chat_lines::gap_before;
use super::text::display_width;
use crate::{
    animation::AURA_WIDTH,
    app::{AgentsTab, HoverTarget, PanelFocus, ProjectListRow},
    App, Theme,
};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

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

    let padded = centered_text_area(inner);
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
    let spinner = app.spinner_char();
    let hovered_activity = match app.hover() {
        Some(HoverTarget::Activity(id)) => Some(id),
        _ => None,
    };
    let (messages, cache) = app.chat_line_cache();
    let reflowed = cache.update(messages, padded.width as usize, theme, spinner);
    let mut activity_headers = Vec::new();
    let mut message_rows = 0;
    for (i, message) in messages.iter().enumerate() {
        message_rows += usize::from(gap_before(messages, i));
        if message.activity_step().is_some() {
            activity_headers.push((message_rows, message.version().0));
        }
        message_rows += cache.lines(message).len();
    }
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
    app.set_chat_layout(activity_headers, start);

    let (messages, cache) = app.chat_line_cache();
    let blank = Line::default();
    let height = padded.height as usize;
    let mut visible = Vec::with_capacity(height);
    let mut line_index = 0usize;
    for (i, message) in messages.iter().enumerate() {
        if gap_before(messages, i) {
            push_visible(&mut visible, &mut line_index, start, height, blank.clone());
        }
        let hover_header =
            message.activity_step().is_some() && hovered_activity == Some(message.version().0);
        for (offset, line) in cache.lines(message).iter().enumerate() {
            if line_index >= start && visible.len() < height {
                let mut line = line.clone();
                if hover_header && offset == 0 {
                    apply_activity_hover(&mut line, theme);
                }
                visible.push(line);
            }
            line_index += 1;
        }
    }
    if let Some(line) = working_line {
        if line_index > 0 {
            push_visible(&mut visible, &mut line_index, start, height, blank);
        }
        push_visible(&mut visible, &mut line_index, start, height, line.clone());
    }
    frame.render_widget(Paragraph::new(visible), padded);
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
    if app.has_live_activity() {
        return None;
    }
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

/// Hint shown at the bottom of the roster tab.
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
    if body.height == 0 {
        return;
    }
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
            let mut lines = build_swarm_lines(app, inner, theme);
            let new_project = lines.pop().unwrap_or_default();
            let lines: Vec<Line> = lines
                .into_iter()
                .skip(app.agents_scroll())
                .take(visible)
                .collect();
            frame.render_widget(Paragraph::new(lines), rows_area);
            frame.render_widget(Paragraph::new(new_project), bottom_row);
        }
        AgentsTab::Roster => {
            let show_hint = body.height >= 3;
            let rows_area = Rect {
                height: body.height - u16::from(show_hint),
                ..body
            };
            let visible = rows_area.height as usize;
            app.set_definition_rows_area(rows_area);
            app.sync_definitions_scroll(visible);
            let lines: Vec<Line> = build_definition_lines(app, inner, theme)
                .into_iter()
                .skip(app.definitions_scroll())
                .take(visible)
                .collect();
            frame.render_widget(Paragraph::new(lines), rows_area);
            if show_hint {
                let hint =
                    truncate_agent_name(ROSTER_HINT, (inner.width as usize).saturating_sub(1));
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        format!(" {hint}"),
                        Style::default().fg(theme.colors.muted),
                    ))),
                    bottom_row,
                );
            }
        }
    }
}

/// The tab labels, and below them a rule that is heavy under the active tab.
fn build_tab_lines<'a>(app: &App, width: usize, theme: &Theme) -> Vec<Line<'a>> {
    let active = app.agents_tab();
    let hovered = match app.hover() {
        Some(HoverTarget::AgentTab(tab)) => Some(tab),
        _ => None,
    };
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
        } else if hovered == Some(tab) {
            labels.push(Span::styled(
                format!(" {} ", tab.label()),
                hover_style(theme),
            ));
            rule.push(Span::styled("─".repeat(tab_width), muted));
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

/// A project is its own row under a folder icon, with its first agent
/// below it and agents that agent spawned indented further. Each agent
/// shows its status icon in the folder's column and how long ago it was
/// last active on the right. A ring spins beside agents that are thinking.
/// Space folds the project and hides every agent under it. A project is a
/// gray cell two lines tall, drawn with half blocks. The last line is
/// "+ New project", which the panel pins to its bottom row:
///
/// ```text
/// ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄
///  ▱  Website
/// ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
///  ⢎⡱ Agent              1m
///    · reviewer          2d
///
/// ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄
///  ▰  Research
/// ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
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
    let mut lines = Vec::new();

    for (row_index, row) in app.project_rows().into_iter().enumerate() {
        let is_selected = row_index == selected && is_focused;
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
                let state = CellState::of(is_selected, false, false);
                let cell = cell_style(theme, state, theme.colors.foreground, true);
                lines.push(cell_edge(width, "▄", cell));
                lines.push(list_row(
                    width,
                    0,
                    icon,
                    &project_label(&app.agents()[root]),
                    cell,
                    None,
                ));
                lines.push(cell_edge(width, "▀", cell));
            }
            ProjectListRow::Agent { index } => {
                let agent = &app.agents()[index];
                let active = agent.id == active_id;
                let icon = if is_thinking(agent) {
                    aura_icon(app)
                } else if active {
                    StatusIcon::new("●", theme.colors.secondary)
                } else {
                    StatusIcon::new("·", theme.colors.muted)
                };
                let ago = agent.last_active_ms.map(|ms| format_ago(ms, now_ms));
                let state = CellState::of(is_selected, is_hovered, active);
                lines.push(list_row(
                    width,
                    agent.depth,
                    icon,
                    &agent.name,
                    cell_style(theme, state, theme.colors.muted, false),
                    ago.as_deref(),
                ));
            }
            ProjectListRow::NewProject => {
                let cell = cell_style(
                    theme,
                    CellState::of(is_selected, is_hovered, false),
                    theme.colors.muted,
                    false,
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
struct StatusIcon {
    glyph: String,
    color: Color,
}

impl StatusIcon {
    fn new(glyph: &str, color: Color) -> Self {
        Self {
            glyph: glyph.to_string(),
            color,
        }
    }
}

fn aura_icon(app: &App) -> StatusIcon {
    let (glyph, color) = app.aura_frame();
    StatusIcon { glyph, color }
}

/// ` icon name      time `, filled with the row's background (if it has
/// one) from one panel border to the other. The icon column is wide enough
/// for the aura ring, so names line up whatever icon a row shows. `depth`
/// indents the row two columns per level.
fn list_row<'a>(
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

/// A half line of `cell`'s background across the panel: `▄` above the name
/// line and `▀` below it make the cell two lines tall with half a line of
/// panel background between neighboring cells.
fn cell_edge<'a>(width: usize, half: &str, cell: CellStyle) -> Line<'a> {
    let style = cell
        .bg
        .map_or_else(Style::default, |bg| Style::default().fg(bg));
    Line::from(Span::styled(half.repeat(width), style))
}

/// How a list cell is drawn, from strongest to weakest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CellState {
    /// The keyboard cursor, while the panel is focused.
    Selected,
    /// Under the pointer.
    Hovered,
    /// The agent shown in the chat.
    Active,
    Normal,
}

impl CellState {
    const fn of(selected: bool, hovered: bool, active: bool) -> Self {
        if selected {
            Self::Selected
        } else if hovered {
            Self::Hovered
        } else if active {
            Self::Active
        } else {
            Self::Normal
        }
    }
}

/// Colors for one list row. `icon` overrides the icon's own color.
#[derive(Debug, Clone, Copy)]
struct CellStyle {
    bg: Option<Color>,
    text: Style,
    icon: Option<Color>,
    time: Color,
}

/// `filled` rows (projects) sit on a dark gray cell that lightens under the
/// keyboard cursor; the rest stay on the panel background and turn bright
/// blue with dark text when selected. `text` is the row's name color when
/// nothing else applies.
fn cell_style(theme: &Theme, state: CellState, text: Color, filled: bool) -> CellStyle {
    let colors = &theme.colors;
    let plain = |bg: Option<Color>, text: Style| CellStyle {
        bg,
        text,
        icon: None,
        time: colors.muted,
    };
    match state {
        CellState::Selected if filled => plain(
            Some(colors.surface_hover),
            Style::default()
                .fg(colors.foreground)
                .add_modifier(Modifier::BOLD),
        ),
        CellState::Selected => CellStyle {
            bg: Some(colors.selection),
            text: Style::default()
                .fg(colors.on_selection)
                .add_modifier(Modifier::BOLD),
            icon: Some(colors.on_selection),
            time: colors.on_selection,
        },
        CellState::Hovered => plain(None, hover_style(theme)),
        CellState::Active => plain(
            filled.then_some(colors.surface),
            Style::default().fg(colors.foreground),
        ),
        CellState::Normal => plain(filled.then_some(colors.surface), Style::default().fg(text)),
    }
}

fn now_ms() -> u64 {
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

/// Roster rows: "@  name" for each permanent agent, then "+  New agent",
/// with the markers and the names in the same columns as the projects list.
fn build_definition_lines<'a>(app: &App, inner: Rect, theme: &Theme) -> Vec<Line<'a>> {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let selected = app.selected_definition();
    let width = inner.width as usize;
    let names = app
        .definitions()
        .iter()
        .map(|definition| ("@", definition.name.as_str(), theme.colors.secondary))
        .chain(std::iter::once(("+", "New agent", theme.colors.muted)));

    names
        .enumerate()
        .map(|(i, (marker, name, text))| {
            let is_selected = i == selected && is_focused;
            let is_hovered =
                matches!(app.hover(), Some(HoverTarget::DefinitionRow(row)) if row == i);
            let state = CellState::of(is_selected, is_hovered, false);
            let cell = cell_style(theme, state, text, false);
            let icon = StatusIcon::new(marker, cell.text.fg.unwrap_or(text));
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
