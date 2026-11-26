//! Where the content columns go: the Agents, Chat and right columns side
//! by side, and the Record and Sandbox panels stacked in the right one.

use crate::App;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Whether the right column (Record above Sandbox) has anything to show.
fn right_column_visible(app: &App) -> bool {
    app.record_panel_visible() || app.sandbox_panel_visible()
}

/// Split `area` into the Agents, Chat and right columns. Only `area`'s
/// width decides where the columns go.
///
/// The Agents column's width never depends on whether the right column
/// is shown: opening or closing the right column only moves the Chat
/// panel's right edge.
pub(super) fn columns(area: Rect, app: &App) -> (Option<Rect>, Rect, Option<Rect>) {
    let swarm_visible = app.swarm_panel_visible();
    let right_visible = right_column_visible(app);
    let splits = app.pane_splits();

    let left = swarm_visible.then(|| agents_width(area.width, splits.agents));
    let used = left.map_or(0, |left| left + GUTTER);
    let right = right_visible.then(|| {
        // What the Agents column and the Chat panel leave over.
        let room = area.width.saturating_sub(used + MIN_CHAT_WIDTH + GUTTER);
        let default = if swarm_visible {
            area.width * 24 / 100
        } else {
            area.width * 35 / 100
        };
        splits
            .right
            .map_or(default.min(room.max(MIN_RIGHT_WIDTH)), |split| {
                dragged(area.width, split).min(room)
            })
    });

    let mut constraints = Vec::with_capacity(3);
    constraints.extend(left.map(Constraint::Length));
    constraints.push(Constraint::Fill(1));
    constraints.extend(right.map(Constraint::Length));
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .spacing(GUTTER)
        .constraints(constraints)
        .split(area);
    let swarm = left.map(|_| chunks[0]);
    let chat = chunks[usize::from(swarm_visible)];
    let right = right.map(|_| chunks[chunks.len() - 1]);
    (swarm, chat, right)
}

/// Width of the Agents column in a `width`-wide row. A dragged divider
/// leaves room for the Chat panel and a narrow right column beside it
/// whether or not the right column is shown, so showing it never moves
/// this column.
fn agents_width(width: u16, split: Option<f32>) -> u16 {
    let room = width.saturating_sub(MIN_CHAT_WIDTH + MIN_RIGHT_WIDTH + 2 * GUTTER);
    split.map_or(width * 24 / 100, |split| dragged(width, split).min(room))
}

/// Columns the Chat panel keeps however far the dividers are dragged.
const MIN_CHAT_WIDTH: u16 = 30;

/// Columns the Agents column leaves for the right column, shown or not,
/// so opening it does not push the Agents column's divider.
const MIN_RIGHT_WIDTH: u16 = 20;

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
pub(super) fn split_right_column(right: Option<Rect>, app: &App) -> (Option<Rect>, Option<Rect>) {
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
