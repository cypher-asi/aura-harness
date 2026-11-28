//! What a channel member's second row says while it starts or after it
//! failed, and the whole of it when the row cuts it off: under the row
//! while the pointer is over it or the keyboard has it selected.

use super::text::{display_width, wrap_words};
use crate::app::{App, HoverTarget, PanelFocus, CHANNEL_MEMBER_ROWS};
use crate::events::AgentSummary;
use crate::Theme;
use ratatui::{layout::Rect, style::Style, Frame};

/// Most lines the reason takes.
const MAX_LINES: usize = 8;

/// The member the tooltip is for: the one under the pointer, else the
/// selected one while the panel has the keyboard.
fn pointed_member(app: &App) -> Option<usize> {
    match app.hover() {
        Some(HoverTarget::ChannelMemberRow(index)) => Some(index),
        _ if app.focus() == PanelFocus::Sandbox => Some(app.selected_channel_member()),
        _ => None,
    }
}

/// What a member starting on its cloud machine is doing: the step its
/// start panel is at, its place in the plan and for how long, those first
/// so a narrow row keeps them; else that it starts.
pub(super) fn starting_status(member: &AgentSummary) -> String {
    let Some(step) = &member.start_step else {
        return super::channel_side::STARTING.to_string();
    };
    let since = aura_core_types::time::now_ms().saturating_sub(step.since_ms);
    let elapsed = super::chat_panel::format_elapsed(std::time::Duration::from_millis(since));
    format!(
        "Starting {}/{} · {elapsed} · {}",
        step.index, step.total, step.label
    )
}

/// The whole of what `member`'s row says when it failed or starts.
fn full_status(member: &AgentSummary) -> Option<String> {
    if let Some(why) = &member.error {
        return Some(format!("Failed: {why}"));
    }
    member.starting.then(|| starting_status(member))
}

/// Draw the whole reason of the pointed-at member's failure over the
/// member rows in `rows`, `first` of the members `shown` scrolled off
/// above, when its row is too narrow for it.
pub(super) fn render_member_tooltip(
    frame: &mut Frame,
    rows: Rect,
    (members, shown, first): (&[AgentSummary], &[usize], usize),
    app: &App,
    theme: &Theme,
) {
    let Some(index) = pointed_member(app) else {
        return;
    };
    let (Some(member), Some(at)) = (members.get(index), shown.iter().position(|&i| i == index))
    else {
        return;
    };
    let Some(text) = full_status(member) else {
        return;
    };
    let width = usize::from(rows.width);
    if at < first || width < 8 || display_width(&text) + 4 <= width {
        return;
    }
    let lines: Vec<String> = wrap_words(&text, width - 2)
        .into_iter()
        .take(MAX_LINES)
        .collect();
    let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let row_top = u16::try_from((at - first) * CHANNEL_MEMBER_ROWS).unwrap_or(u16::MAX);
    let below = rows.y.saturating_add(row_top).saturating_add(2);
    let y = if below.saturating_add(height) <= rows.bottom() {
        below
    } else {
        rows.y
            .saturating_add(row_top)
            .saturating_sub(height)
            .max(rows.y)
    };
    let color = if member.error.is_some() {
        super::failure_color(theme)
    } else {
        theme.colors.foreground
    };
    let style = Style::default().bg(theme.colors.surface_hover).fg(color);
    let buf = frame.buffer_mut();
    for (offset, line) in (0u16..).zip(&lines) {
        let row = y + offset;
        if row >= rows.bottom() || row >= buf.area.bottom() {
            break;
        }
        let area = Rect::new(rows.x, row, rows.width, 1);
        buf.set_style(area, style);
        buf.set_stringn(rows.x + 1, row, line, width - 2, style);
    }
}
