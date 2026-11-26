//! What every panel is drawn in: its bordered body block and its
//! scrollbar.

use crate::Theme;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};

/// Draw a panel's bordered body in `area`, below its header bar, returning
/// the area inside the border when it has any rows. A `hosted` bar is
/// open at the bottom, so the body's top border joins its sides.
pub(super) fn render_body_block(
    frame: &mut Frame,
    area: Rect,
    focus: (bool, bool),
    theme: &Theme,
) -> Option<Rect> {
    let block = body_block(focus, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    (inner.height > 0).then_some(inner)
}

/// The border of a panel's body, below its header bar.
pub(super) fn body_block<'a>((is_focused, hosted): (bool, bool), theme: &Theme) -> Block<'a> {
    let border_color = border_color(is_focused, theme);
    let border_set = if hosted {
        ratatui::symbols::border::Set {
            top_left: "├",
            top_right: "┤",
            ..ratatui::symbols::border::PLAIN
        }
    } else {
        ratatui::symbols::border::PLAIN
    };
    Block::default()
        .borders(Borders::ALL)
        .border_set(border_set)
        .border_style(Style::default().fg(border_color))
}

/// A panel's border: the accent while it has focus, dim gray otherwise.
pub(super) const fn border_color(is_focused: bool, theme: &Theme) -> Color {
    if is_focused {
        theme.colors.success
    } else {
        theme.colors.faint
    }
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
    let Some(column) = scrollbar_track(panel, rows, view) else {
        return;
    };
    let border = border_color(is_focused, theme);
    let thumb = if is_focused {
        theme.colors.foreground
    } else {
        theme.colors.muted
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

/// The column [`render_scrollbar`] draws on, when it draws one.
pub(super) fn scrollbar_track(panel: Rect, rows: Rect, view: ScrollView) -> Option<Rect> {
    (view.total > view.visible && panel.width > 0 && rows.height > 0).then(|| Rect {
        x: panel.x + panel.width - 1,
        y: rows.y,
        width: 1,
        height: rows.height,
    })
}
