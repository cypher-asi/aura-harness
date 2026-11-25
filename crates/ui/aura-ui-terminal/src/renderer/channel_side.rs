//! The side panel's PROJECTS tab, listed while a channel is open: the
//! projects linked to the channel, the members at work now, and every
//! member's task in the TASKS tab's tree.
//!
//! ```text
//!  LINKED PROJECTS
//!     Zode  ~/code/zode
//!
//!  WORKING NOW
//!  ⢎⡱ CO  Coordinator
//!
//!  TASKS
//!  ▾  No Plan
//!  ✓  Fix the typo  → coder
//! ```
//!
//! The two lists above the tasks take the rows they need, cut to the
//! panel; the tasks scroll in the rows left below them.

use super::panels::{
    cell_style, list_row, list_row_with_detail, working_icon, CellState, StatusIcon,
};
use super::policies::heading;
use super::side_list::{empty_line, render_empty, render_hint, split_hint, CHANNEL_HINT};
use super::tasks::{render_task_tree, TreeView};
use crate::app::PanelFocus;
use crate::events::ChannelSummary;
use crate::{App, Theme};
use ratatui::{layout::Rect, style::Style, text::Line, widgets::Paragraph, Frame};

/// Draw the open channel into `inner`, the body below the tab bar of the
/// panel bordered by `area`. Without an open channel there is nothing
/// to draw.
pub(super) fn render_channel_side(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let Some(channel) = app.open_channel().cloned() else {
        return;
    };
    let is_focused = app.focus() == PanelFocus::Sandbox;
    let (body, hint) = split_hint(inner, is_focused);
    let mut lines = summary_lines(&channel, body.width, app, theme);
    lines.push(heading("TASKS", None, theme));
    let top_height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .min(body.height);
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            height: top_height,
            ..body
        },
    );

    let rows_area = Rect {
        y: body.y + top_height,
        height: body.height - top_height,
        ..body
    };
    if rows_area.height > 0 {
        if app.channel_task_list().rows().is_empty() {
            render_empty(frame, rows_area, "No tasks yet.", theme);
        } else {
            let first = app.sync_channel_tasks_scroll(usize::from(rows_area.height));
            let view = TreeView {
                first,
                focused: is_focused,
                hovered: None,
            };
            let app = &*app;
            render_task_tree(
                frame,
                (area, rows_area),
                app.channel_task_list(),
                view,
                app,
                theme,
            );
        }
    }
    if let Some(hint) = hint {
        render_hint(frame, hint, CHANNEL_HINT, theme);
    }
}

/// The `LINKED PROJECTS` and `WORKING NOW` sections, each followed by a
/// blank line, `width` columns wide.
fn summary_lines(
    channel: &ChannelSummary,
    width: u16,
    app: &App,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let columns = usize::from(width);
    let plain = cell_style(theme, CellState::Normal, Style::default());
    let mut lines = vec![heading("LINKED PROJECTS", None, theme)];
    if channel.projects.is_empty() {
        lines.push(empty_line("No linked projects", width, theme));
    }
    lines.extend(channel.projects.iter().map(|project| {
        list_row_with_detail(
            columns,
            0,
            StatusIcon::new("", theme.colors.muted),
            (&project.name, Some(&project.folder)),
            plain,
            None,
        )
    }));
    lines.push(Line::default());

    lines.push(heading("WORKING NOW", None, theme));
    let mut working = channel
        .members
        .iter()
        .filter(|member| member.running)
        .peekable();
    if working.peek().is_none() {
        lines.push(empty_line("Nobody is working", width, theme));
    }
    lines.extend(working.map(|member| {
        let name = match member.code.as_deref() {
            Some(code) => format!("{code}  {}", member.name),
            None => member.name.clone(),
        };
        list_row(columns, 0, working_icon(app, theme), &name, plain, None)
    }));
    lines.push(Line::default());
    lines
}
