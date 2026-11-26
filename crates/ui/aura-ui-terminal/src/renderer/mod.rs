//! Simple IRC-style renderer for the terminal UI.

mod activity;
mod agents;
mod bench_panel;
mod benchmarks;
mod boot;
mod channel_form;
mod channel_side;
mod chat_lines;
mod chat_panel;
mod chrome;
mod command;
mod context_menu;
mod detail;
mod diff;
mod flow;
mod flow_minimap;
mod flow_timeline;
mod graph;
mod header;
mod host_tab;
mod hud;
mod images;
mod inbox;
mod input;
mod markdown;
mod members;
mod mention_menu;
mod overlays;
mod plan_doc;
mod plans;
mod policies;
mod processes;
mod queue_panel;
mod record_panel;
mod run_log;
mod sandbox;
mod segments;
mod side_list;
mod slash_menu;
mod tasks;
mod text;
mod trace_feed;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

pub(crate) use crate::components::{format_ago, now_ms};
pub(crate) use chat_lines::{ChatLineCache, GroupState};
pub(crate) use chat_panel::format_elapsed;

use crate::mode_command::ApprovalLevel;
use crate::{app::PanelAreas, App, Theme};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Columns that text outside the panels keeps from the screen edges.
/// Border glyphs are drawn through the middle of their cell, so text in the
/// outermost column would overhang the panel borders.
pub(super) const EDGE_INSET: u16 = 1;

/// Rows of a block that show more of a part cut short when clicked, each
/// counted from the block's first line.
type MoreRows = Vec<(usize, crate::components::MoreSection)>;

/// `… 340 more lines · click to show 200 more`, for `hidden` lines cut.
fn more_label(hidden: usize) -> String {
    let page = hidden.min(crate::components::MORE_PAGE_LINES);
    format!("… {hidden} more lines · click to show {page} more")
}

fn inset_edges(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(EDGE_INSET),
        width: area.width.saturating_sub(EDGE_INSET * 2),
        ..area
    }
}

/// Render the full application UI: the header row, then the panels, with
/// the input at the bottom of the Chat panel and the footer under it. The
/// side columns end level with the input.
pub fn render(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3)])
        .split(area);
    let content = inset_edges(rows[1]);

    // The input grows upward as it wraps inside the Chat column, so its
    // height comes from where the columns fall.
    let (swarm, column, right) = columns(content, app);
    // The queue and benchmark panels give up rows before the chat does:
    // the chat keeps three and the input one. The queue goes first.
    let queue_rows = queue_panel::height(app).min(
        content
            .height
            .saturating_sub(hud::FOOTER_ROWS + input::BOX_ROWS + 1 + 3),
    );
    let bench_rows = bench_panel::height(app).min(
        content
            .height
            .saturating_sub(hud::FOOTER_ROWS + input::BOX_ROWS + 1 + 3 + queue_rows),
    );
    let max_input_rows = content
        .height
        .saturating_sub(hud::FOOTER_ROWS + input::BOX_ROWS + 3 + queue_rows + bench_rows)
        .max(1);
    let input_rows = input::input_rows(column, app, max_input_rows);
    let chat_column = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(bench_rows),
            Constraint::Length(queue_rows),
            Constraint::Length(input_rows + input::BOX_ROWS),
            Constraint::Length(hud::FOOTER_ROWS),
        ])
        .split(column);
    let above_footer = |side: Rect| Rect {
        height: side.height.saturating_sub(hud::FOOTER_ROWS),
        ..side
    };
    let under_right = right.map(|side| Rect {
        y: side.bottom().saturating_sub(hud::FOOTER_ROWS),
        height: hud::FOOTER_ROWS.min(side.height),
        ..side
    });
    let (swarm, right) = (swarm.map(above_footer), right.map(above_footer));

    // The panels' outer borders run through the middle of `content`'s
    // edge columns, so the header keeps a column in from those to stay
    // inside them.
    let header = inset_edges(Rect {
        x: content.x,
        width: content.width,
        ..rows[0]
    });
    app.set_prose_rows(Vec::new());
    app.clear_chat_scroll();
    let chat = render_content_panels(frame, (swarm, chat_column[0], right), app, theme);
    // After the panels, which start this frame's clickable areas afresh.
    render_header(frame, header, app, theme);
    images::render_pending_images(frame, chat, app, theme);
    detail::render_detail_view(frame, chat, app, theme);
    app.track_selection(frame.buffer_mut());
    if let Some(selection) = app.selection() {
        selection.highlight(frame.buffer_mut());
    }
    bench_panel::render(frame, chat_column[1], app, theme);
    queue_panel::render(frame, chat_column[2], app, theme);
    app.set_input_area(chat_column[3]);
    let drawn = input::render_input(frame, chat_column[3], app, theme);
    app.set_input_stop_area(drawn.stop);
    app.set_input_wrap_width(drawn.wrap_width);
    app.set_input_rows(drawn.rows);
    let footer_gap = hud::render_footer(frame, inset_edges(chat_column[4]), app, theme);
    app.set_footer_area(inset_edges(chat_column[4]));
    let (records, sandbox) = split_right_column(right, app);
    let notice_area = under_right.unwrap_or(footer_gap);
    if let Some(read) = hud::render_notice(frame, (notice_area, sandbox.or(records)), app, theme) {
        app.hold_notification_for(read);
    }

    hud::render_todo_panel(frame, chat, app, theme);
    slash_menu::render_slash_menu(frame, chat, app, theme);
    mention_menu::render_mention_menu(frame, chat, app, theme);
    context_menu::render_context_menu(frame, app, theme);
    overlays::render_folder_picker(frame, app, theme);
    channel_form::render_channel_form(frame, app, theme);
    overlays::render_overlays(frame, app, theme);
    app.drop_covered_prose(frame.buffer_mut());
    app.drop_covered_code_copies(frame.buffer_mut());
    app.drop_covered_chat(frame.buffer_mut());
    app.trim_covered_scrollbar(frame.buffer_mut());
    app.flow_mut().take_canvas_marks(frame.buffer_mut());
}

/// Whether the right column (Record above Sandbox) has anything to show.
fn right_column_visible(app: &App) -> bool {
    app.record_panel_visible() || app.sandbox_panel_visible()
}

/// Render the content panels (Swarm, Chat, Record, Sandbox) in the columns
/// [`columns`] gave them, returning where the Chat panel's body was drawn,
/// below its header bar.
fn render_content_panels(
    frame: &mut Frame,
    (swarm, chat, right): (Option<Rect>, Rect, Option<Rect>),
    app: &mut App,
    theme: &Theme,
) -> Rect {
    app.clear_pane_chrome();
    app.flow_mut().set_view(None, false);
    let (records, sandbox) = split_right_column(right, app);
    let areas = PanelAreas {
        swarm,
        chat: Some(chat),
        records,
        sandbox,
        ..PanelAreas::default()
    };
    app.set_panel_areas(areas);

    if let Some(swarm) = areas.swarm {
        agents::render_agents_panel(frame, swarm, app, theme);
    }
    if let Some(chat) = areas.chat {
        chat_panel::render_chat_panel(frame, chat, app, theme);
    }
    if let Some(records) = areas.records {
        record_panel::render_record_panel(frame, records, app, theme);
    }
    if let Some(sandbox) = areas.sandbox {
        sandbox::render_sandbox_panel(frame, sandbox, app, theme);
    }
    header::split(chat, app.host_chrome()).1
}

/// Split `area` into the Agents, Chat and right columns. Only `area`'s
/// width decides where the columns go.
fn columns(area: Rect, app: &App) -> (Option<Rect>, Rect, Option<Rect>) {
    let swarm_visible = app.swarm_panel_visible();
    let right_visible = right_column_visible(app);
    let splits = app.pane_splits();

    let gutters = GUTTER * (u16::from(swarm_visible) + u16::from(right_visible));
    let room = area.width.saturating_sub(MIN_CHAT_WIDTH + gutters);
    let side = |split: Option<f32>, percent: u16| match split {
        Some(split) => [
            Constraint::Length(dragged(area.width, split).min(room)),
            Constraint::Fill(1),
        ],
        None => [
            Constraint::Percentage(percent),
            Constraint::Percentage(100 - percent),
        ],
    };
    match (swarm_visible, right_visible) {
        (true, true) => {
            let default = area.width * 24 / 100;
            let left = splits
                .agents
                .map_or(default, |split| dragged(area.width, split).min(room / 2));
            let right = splits.right.map_or(default, |split| {
                dragged(area.width, split).min(room.saturating_sub(left))
            });
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .spacing(GUTTER)
                .constraints([
                    Constraint::Length(left),
                    Constraint::Fill(1),
                    Constraint::Length(right),
                ])
                .split(area);
            (Some(chunks[0]), chunks[1], Some(chunks[2]))
        }
        (true, false) => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .spacing(GUTTER)
                .constraints(side(splits.agents, 25))
                .split(area);
            (Some(chunks[0]), chunks[1], None)
        }
        (false, true) => {
            let [right, chat] = side(splits.right, 35);
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .spacing(GUTTER)
                .constraints([chat, right])
                .split(area);
            (None, chunks[0], Some(chunks[1]))
        }
        (false, false) => (None, area, None),
    }
}

/// Columns the Chat panel keeps however far the dividers are dragged.
const MIN_CHAT_WIDTH: u16 = 30;

/// Blank columns between panels side by side. Cells are about twice as
/// tall as they are wide, so one column matches the gap between panels
/// stacked on rows that touch.
const GUTTER: u16 = 1;

/// Rows each panel of the right column keeps when they share it.
const MIN_STACKED_HEIGHT: u16 = 6;

/// The cells a divider dragged to `split` of `total` leaves before it.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn dragged(total: u16, split: f32) -> u16 {
    (f32::from(total) * split.clamp(0.0, 1.0)).round() as u16
}

/// Record on top and Sandbox below, halves unless the divider between
/// them was moved; either one takes the whole column when the other is
/// hidden.
fn split_right_column(right: Option<Rect>, app: &App) -> (Option<Rect>, Option<Rect>) {
    let Some(area) = right else {
        return (None, None);
    };
    match (app.record_panel_visible(), app.sandbox_panel_visible()) {
        (true, true) => {
            let constraints = app.pane_splits().records.map_or(
                [Constraint::Percentage(50), Constraint::Percentage(50)],
                |split| {
                    let keep = MIN_STACKED_HEIGHT.min(area.height / 2);
                    let top = dragged(area.height, split).clamp(keep, area.height - keep);
                    [Constraint::Length(top), Constraint::Fill(1)]
                },
            );
            let halves = Layout::default()
                .direction(Direction::Vertical)
                .constraints(constraints)
                .split(area);
            (Some(halves[0]), Some(halves[1]))
        }
        (true, false) => (Some(area), None),
        (false, true) => (None, Some(area)),
        (false, false) => (None, None),
    }
}

/// Columns of the header's hamburger button: a space, then its glyph in
/// the header's last column, where the text there always ends.
const SIDE_TOGGLE_WIDTH: u16 = 2;

/// The hamburger button's glyph. A front end that draws the chrome
/// leaves the cells blank and draws its own.
const SIDE_TOGGLE_GLYPH: &str = "☰";

/// Render the header bar: the title on the left, the sign-in status and
/// the hamburger button that shows or hides the right column on the right.
fn render_header(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let toggle = (area.width >= 40 && area.height > 0).then(|| {
        Rect::new(
            area.right() - SIDE_TOGGLE_WIDTH,
            area.y,
            SIDE_TOGGLE_WIDTH,
            1,
        )
    });
    app.set_side_toggle_area(toggle);
    if let Some(cell) = toggle {
        if !app.host_chrome() {
            let style = if app.hover() == Some(crate::app::HoverTarget::SideToggle) {
                Style::default()
                    .bg(theme.colors.surface_hover)
                    .fg(theme.colors.foreground)
            } else {
                Style::default().fg(theme.colors.muted)
            };
            let buf = frame.buffer_mut();
            buf.set_style(cell, style);
            buf.set_string(cell.x + 1, cell.y, SIDE_TOGGLE_GLYPH, style);
        }
    }
    // The status keeps a column of space from the button.
    let area = Rect {
        width: area
            .width
            .saturating_sub(toggle.map_or(0, |cell| cell.width + 1)),
        ..area
    };

    let status_spans = header_status_spans(app, theme);
    let status_width = spans_width(&status_spans);
    let right_width = u16::try_from(status_width.max(30) + 1).unwrap_or(u16::MAX);

    let header_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(right_width)])
        .split(area);

    let title = Line::from(vec![
        Span::styled("AURA", Style::default().fg(theme.colors.foreground)),
        Span::styled(" CLI", Style::default().fg(theme.colors.muted)),
    ]);
    frame.render_widget(Paragraph::new(title), header_layout[0]);

    let right = Line::from(status_spans);
    frame.render_widget(
        Paragraph::new(right).alignment(Alignment::Right),
        header_layout[1],
    );
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| text::display_width(&s.content)).sum()
}

/// `● local` when signed out, otherwise the one sign-in that serves the
/// chat's model: `● CLAUDE MAX 20X`.
/// It follows the model as it changes; when no sign-in serves it (a local
/// model, say) the first one is shown. The dot reflects API connectivity.
fn header_status_spans<'a>(app: &'a App, theme: &Theme) -> Vec<Span<'a>> {
    let (icon, icon_color) = if app.api_active() {
        ("● ", theme.colors.success)
    } else {
        ("○ ", theme.colors.error)
    };
    let muted = Style::default().fg(theme.colors.muted);
    let signed_in = Style::default().fg(theme.colors.primary);

    // Each sign-in, tagged with the provider id it serves models for.
    let mut segments: Vec<(Option<&str>, Vec<Span<'a>>)> = Vec::new();
    if let Some(user) = app.user() {
        segments.push((
            Some("zos"),
            vec![Span::styled(user, signed_in), Span::styled(" (zOS)", muted)],
        ));
    }
    for plan in app.plans() {
        segments.push((
            plan.split_whitespace().next(),
            vec![Span::styled(plan.to_uppercase(), signed_in)],
        ));
    }
    if segments.is_empty() {
        segments.push((None, vec![Span::styled("local", muted)]));
    }

    let active = current_provider(app)
        .and_then(|provider| {
            segments
                .iter()
                .position(|(id, _)| id.is_some_and(|id| id.eq_ignore_ascii_case(provider)))
        })
        .unwrap_or(0);

    let mut spans = vec![Span::styled(icon, Style::default().fg(icon_color))];
    if let Some((_, segment)) = segments.into_iter().nth(active) {
        spans.extend(segment);
    }
    spans
}

/// Provider id (`zos`, `claude`, `chatgpt`, `openrouter`) that serves the
/// chat's model. Under Gate it is the one serving Gate's next pick, read
/// from the route label, e.g. `claude-sonnet-4-6 (claude plan)`.
fn current_provider(app: &App) -> Option<&str> {
    if app.current_model() == "gate" {
        let (_, backend) = app.model_route()?.rsplit_once('(')?;
        return match backend.split_whitespace().next()?.trim_end_matches(')') {
            "cloud" => Some("zos"),
            id => Some(id),
        };
    }
    app.current_model_option()?.provider.as_deref()
}

/// `high thinking`: the thinking level pinned for every turn, brighter
/// than `auto thinking`, when each model picks its own. Empty until the
/// kernel reports the model.
pub(super) fn thinking_indicator_spans<'a>(app: &App, theme: &Theme) -> Vec<Span<'a>> {
    if app.current_model().is_empty() {
        return Vec::new();
    }
    let (level, color) = app
        .thinking()
        .map_or(("auto", theme.colors.muted), |level| {
            (level, theme.colors.foreground)
        });
    vec![Span::styled(
        format!("{level} thinking"),
        Style::default().fg(color),
    )]
}

/// `ASK · qwen3:4b`: the approval level, then the model, or `GATE` when
/// Gate picks it. Each is left out until the kernel reports it.
pub(super) fn model_indicator_spans<'a>(app: &'a App, theme: &Theme) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let separator = || Span::styled(" · ", Style::default().fg(theme.colors.faint));
    if let Some(level) = app.approval() {
        let (label, color) = match level {
            ApprovalLevel::Yolo => ("YOLO", theme.colors.foreground),
            ApprovalLevel::Ask => ("ASK", theme.colors.muted),
            ApprovalLevel::Strict => ("STRICT", theme.colors.foreground),
        };
        spans.push(Span::styled(label, Style::default().fg(color)));
    }
    if app.current_model().is_empty() {
        return spans;
    }
    if !spans.is_empty() {
        spans.push(separator());
    }
    if app.current_model() == "gate" {
        spans.push(Span::styled(
            "GATE",
            Style::default().fg(theme.colors.foreground),
        ));
        return spans;
    }
    let label = app
        .current_model_option()
        .map_or_else(|| app.current_model(), |m| m.label.as_str());
    spans.push(Span::styled(label, Style::default().fg(theme.colors.muted)));
    spans
}
