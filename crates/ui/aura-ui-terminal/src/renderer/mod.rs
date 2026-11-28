//! Simple IRC-style renderer for the terminal UI.

mod activity;
mod agents;
mod bench_panel;
mod benchmarks;
mod boot;
mod channel_history;
mod channel_queue;
mod channel_side;
mod channel_welcome;
mod chat_lines;
mod chat_panel;
mod chrome;
mod columns;
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
mod modals;
mod overlays;
mod plan_doc;
mod plans;
mod policies;
mod processes;
mod queue_panel;
mod record_panel;
mod repo_tab;
mod run_log;
mod sandbox;
mod segments;
mod side_list;
mod slash_menu;
mod tasks;
mod text;
mod todo_drawer;
mod trace_feed;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

pub(crate) use crate::components::format_ago;
pub(crate) use chat_lines::{post_block_key, ChatLineCache, GroupState};
pub(crate) use chat_panel::format_elapsed;

use columns::{columns, split_right_column};

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

/// `… 340 more lines`, for `hidden` lines cut.
fn more_label(hidden: usize) -> String {
    format!("… {hidden} more lines")
}

fn inset_edges(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(EDGE_INSET),
        width: area.width.saturating_sub(EDGE_INSET * 2),
        ..area
    }
}

/// Where this frame's parts go.
struct FrameLayout {
    header: Rect,
    swarm: Option<Rect>,
    right: Option<Rect>,
    /// The Chat column's rows: chat, to-do drawer, benchmarks, queue,
    /// input and footer
    chat_column: [Rect; 6],
    /// The footer row under the right column, when it is shown
    under_right: Option<Rect>,
}

/// The red of an agent that errored out, the Stop button's: its row and
/// icon on PROJECTS, its row on a channel's MEMBERS and the errors in its
/// chat all take it, so a failure reads as the button that stops work.
pub(crate) const fn failure_color(theme: &Theme) -> ratatui::style::Color {
    theme.colors.error
}

/// Render the full application UI: the header row, then the panels, with
/// the input at the bottom of the Chat panel and the footer under it. The
/// side columns end level with the input.
pub fn render(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let layout = frame_layout(frame.area(), app);
    app.set_prose_rows(Vec::new());
    app.clear_chat_scroll();
    app.clear_list_bands();
    let panels = (layout.swarm, layout.chat_column[0], layout.right);
    let chat = render_content_panels(frame, panels, app, theme);
    app.capture_list_bands(frame.buffer_mut());
    // After the panels, which start this frame's clickable areas afresh.
    render_header(frame, layout.header, app, theme);
    render_chat_overlays(frame, chat, app, theme);
    render_below_chat(frame, &layout, app, theme);
    render_popups(frame, chat, app, theme);
    drop_covered(frame, app);
}

/// Lay out the header row, the side columns and the Chat column's rows.
fn frame_layout(area: Rect, app: &App) -> FrameLayout {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3)])
        .split(area);
    let content = inset_edges(rows[1]);

    // The input grows upward as it wraps inside the Chat column, so its
    // height comes from where the columns fall.
    let (swarm, column, right) = columns(content, app);
    let chat_column = chat_column_rows(content, column, app);
    let above_footer = |side: Rect| Rect {
        height: side.height.saturating_sub(hud::FOOTER_ROWS),
        ..side
    };
    let under_right = right.map(|side| Rect {
        y: side.bottom().saturating_sub(hud::FOOTER_ROWS),
        height: hud::FOOTER_ROWS.min(side.height),
        ..side
    });

    // The panels' outer borders run through the middle of `content`'s
    // edge columns, so the header keeps a column in from those to stay
    // inside them.
    let header = inset_edges(Rect {
        x: content.x,
        width: content.width,
        ..rows[0]
    });
    FrameLayout {
        header,
        swarm: swarm.map(above_footer),
        right: right.map(above_footer),
        chat_column,
        under_right,
    }
}

/// Split the Chat `column` into the chat, to-do drawer, benchmarks,
/// queue, input and footer rows.
fn chat_column_rows(content: Rect, column: Rect, app: &App) -> [Rect; 6] {
    // The queue, benchmark and to-do panels give up rows before the chat
    // does: the chat keeps three and the input one. The queue takes its
    // rows first, then the benchmark panel, then the to-do drawer, so the
    // drawer is the first to go short.
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
    let drawer_rows = todo_drawer::height(app).min(
        content
            .height
            .saturating_sub(hud::FOOTER_ROWS + input::BOX_ROWS + 1 + 3 + queue_rows + bench_rows),
    );
    let max_input_rows = content
        .height
        .saturating_sub(
            hud::FOOTER_ROWS + input::BOX_ROWS + 3 + queue_rows + bench_rows + drawer_rows,
        )
        .max(1);
    // The terminal's prompt follows its output inside the chat, so it
    // takes no rows of its own under it.
    let input_rows = if app.terminal_inline() {
        0
    } else {
        input::input_rows(column, app, max_input_rows) + input::BOX_ROWS
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(drawer_rows),
            Constraint::Length(bench_rows),
            Constraint::Length(queue_rows),
            Constraint::Length(input_rows),
            Constraint::Length(hud::FOOTER_ROWS),
        ])
        .split(column);
    [rows[0], rows[1], rows[2], rows[3], rows[4], rows[5]]
}

/// Images, the detail view and the selection over the Chat panel's body.
fn render_chat_overlays(frame: &mut Frame, chat: Rect, app: &mut App, theme: &Theme) {
    images::render_pending_images(frame, chat, app, theme);
    detail::render_detail_view(frame, chat, app, theme);
    app.track_selection(frame.buffer_mut());
    if let Some(selection) = app.selection() {
        selection.highlight(frame.buffer_mut());
    }
}

/// The to-do drawer, the benchmark and queue panels, the input, the
/// footer and the notice. Under a modal the notice is the modal's status
/// instead.
fn render_below_chat(frame: &mut Frame, layout: &FrameLayout, app: &mut App, theme: &Theme) {
    let chat_column = &layout.chat_column;
    todo_drawer::render(frame, chat_column[1], app, theme);
    bench_panel::render(frame, chat_column[2], app, theme);
    queue_panel::render(frame, chat_column[3], app, theme);
    // The terminal's prompt was drawn in the chat panel's body.
    if !app.terminal_inline() {
        input::show_input(frame, chat_column[4], app, theme, false);
    }
    let footer_gap = hud::render_footer(frame, inset_edges(chat_column[5]), app, theme);
    app.set_footer_area(inset_edges(chat_column[5]));
    if app.modal().is_some() {
        return;
    }
    let (records, sandbox) = split_right_column(layout.right, app);
    let notice_area = layout.under_right.unwrap_or(footer_gap);
    if let Some(read) = hud::render_notice(frame, (notice_area, sandbox.or(records)), app, theme) {
        app.hold_notification_for(read);
    }
}

/// The menus, then the modal on top, which hides all of it.
fn render_popups(frame: &mut Frame, chat: Rect, app: &mut App, theme: &Theme) {
    slash_menu::render_slash_menu(frame, chat, app, theme);
    mention_menu::render_mention_menu(frame, chat, app, theme);
    context_menu::render_context_menu(frame, app, theme);
    modals::render_modal_layer(frame, app, theme);
}

/// Drop what was drawn over from this frame's clickable areas, and take
/// the flow canvas's marks from the buffer.
fn drop_covered(frame: &mut Frame, app: &mut App) {
    app.drop_covered_prose(frame.buffer_mut());
    app.cover_list_bands(frame.buffer_mut());
    app.drop_covered_code_copies(frame.buffer_mut());
    app.drop_covered_links(frame.buffer_mut());
    app.drop_covered_chat(frame.buffer_mut());
    app.trim_covered_scrollbar(frame.buffer_mut());
    app.flow_mut().take_canvas_marks(frame.buffer_mut());
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
