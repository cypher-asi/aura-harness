//! What every panel is drawn in: its bordered body block, the padding
//! inside the border, the [`Body`] that holds the tab's rows and the
//! scrollbar on its right border.
//!
//! Every panel keeps [`PAD_TOP`] blank rows under its top border and
//! [`PAD_SIDE`] blank columns inside each side. A list's rows still run
//! from one border to the other, so a selected or hovered row's fill
//! meets them; the rows keep their own blanks at each end instead
//! ([`ROW_INSET`](crate::components::ROW_INSET)).

use crate::Theme;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};

/// Rows a panel keeps blank under its top border.
pub(super) const PAD_TOP: u16 = 1;

/// Columns a panel keeps blank inside each side border, as many as a list
/// row keeps at its ends. A cell is about twice as tall as it is wide, so
/// two columns look as wide as the blank row on top.
pub(super) const PAD_SIDE: u16 = 2;

/// `inner` below its blank top row, where a panel's rows go. A body under
/// three rows has none to spare and keeps them all.
pub(super) const fn pad_top(inner: Rect) -> Rect {
    if inner.height < 3 {
        return inner;
    }
    Rect {
        y: inner.y + PAD_TOP,
        height: inner.height - PAD_TOP,
        ..inner
    }
}

/// `area` inside its blank side columns, where text that is not a list's
/// rows goes. An area too narrow for them keeps its columns.
pub(super) const fn pad_sides(area: Rect) -> Rect {
    if area.width <= 2 * PAD_SIDE {
        return area;
    }
    Rect {
        x: area.x + PAD_SIDE,
        width: area.width - 2 * PAD_SIDE,
        ..area
    }
}

/// `inner` inside the blank row on top and the blank columns at the
/// sides, where a view that is not a list draws.
pub(super) const fn padded(inner: Rect) -> Rect {
    pad_sides(pad_top(inner))
}

/// `lines` laid out as wide as a text area ([`pad_sides`]), with the side
/// padding added at each end so they can be drawn on the full rows. A
/// line whose own style has a fill, as a picked row does, carries it
/// across the padding, so the row is lit from one border to the other.
pub(super) fn inset_lines(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|mut line| {
            let fill = line
                .style
                .bg
                .map_or_else(Style::default, |bg| Style::default().bg(bg));
            let inset = Span::styled(" ".repeat(usize::from(PAD_SIDE)), fill);
            line.spans.insert(0, inset.clone());
            line.spans.push(inset);
            line
        })
        .collect()
}
/// Draw a panel's bordered body in `area`, below its header bar, returning
/// where its rows go, below the blank top row ([`pad_top`]), when it has
/// any. A `hosted` bar is open at the bottom, so the body's top border
/// joins its sides.
pub(super) fn render_body_block(
    frame: &mut Frame,
    area: Rect,
    focus: (bool, bool),
    theme: &Theme,
) -> Option<Rect> {
    let block = body_block(focus, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    (inner.height > 0).then(|| pad_top(inner))
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

/// A panel's body inside its border: the rows its tab draws in and the
/// scrollbar on the panel's right border. Lists and info bodies both draw
/// through one, so every tab scrolls the same way.
#[derive(Debug, Clone, Copy)]
pub(super) struct Body {
    /// The panel, whose right border holds the scrollbar
    panel: Rect,
    /// Where the tab's rows go
    rows: Rect,
    focused: bool,
}

impl Body {
    /// The body of the panel bordered by `panel`, its rows drawn in
    /// `rows`.
    pub(super) const fn new(panel: Rect, rows: Rect, focused: bool) -> Self {
        Self {
            panel,
            rows,
            focused,
        }
    }

    /// Where the rows are drawn.
    pub(super) const fn rows(&self) -> Rect {
        self.rows
    }

    /// Rows that fit.
    pub(super) fn visible(&self) -> usize {
        usize::from(self.rows.height)
    }

    /// Columns of a row.
    pub(super) fn width(&self) -> usize {
        usize::from(self.rows.width)
    }

    /// Columns of a document's lines, inside the side padding.
    pub(super) fn text_width(&self) -> usize {
        usize::from(pad_sides(self.rows).width)
    }

    /// Draw `lines`, the rows on screen from `view.first` on, then the
    /// scrollbar for `view`.
    pub(super) fn draw(
        &self,
        frame: &mut Frame,
        lines: Vec<Line<'_>>,
        view: ScrollView,
        theme: &Theme,
    ) {
        frame.render_widget(Paragraph::new(lines), self.rows);
        render_scrollbar(frame, self.panel, self.rows, view, self.focused, theme);
    }

    /// Draw a whole info document, `lines` [`Body::text_width`] columns
    /// wide, from line `first` (which the tab keeps inside it), inside the
    /// side padding.
    pub(super) fn draw_document(
        &self,
        frame: &mut Frame,
        lines: Vec<Line<'_>>,
        first: usize,
        theme: &Theme,
    ) {
        let view = ScrollView {
            total: lines.len(),
            visible: self.visible(),
            first,
        };
        let shown: Vec<Line<'_>> = lines.into_iter().skip(first).take(view.visible).collect();
        frame.render_widget(Paragraph::new(shown), pad_sides(self.rows));
        render_scrollbar(frame, self.panel, self.rows, view, self.focused, theme);
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn a_body_keeps_a_blank_row_on_top_and_blank_columns_for_text() {
        let inner = Rect::new(1, 1, 20, 6);
        assert_eq!(pad_top(inner), Rect::new(1, 2, 20, 5));
        assert_eq!(pad_sides(inner), Rect::new(3, 1, 16, 6));
        assert_eq!(padded(inner), Rect::new(3, 2, 16, 5));
        let short = Rect::new(1, 1, 20, 2);
        assert_eq!(pad_top(short), short, "no row to spare");
        let narrow = Rect::new(1, 1, 4, 6);
        assert_eq!(pad_sides(narrow), narrow, "no columns to spare");

        let body = Body::new(Rect::new(0, 0, 22, 8), pad_top(inner), true);
        assert_eq!(body.rows(), Rect::new(1, 2, 20, 5), "rows meet the borders");
        assert_eq!(body.visible(), 5);
        assert_eq!(body.width(), 20);
        assert_eq!(body.text_width(), 16);
    }

    #[test]
    fn the_side_padding_is_as_wide_as_a_rows_inset() {
        assert_eq!(usize::from(PAD_SIDE), crate::components::ROW_INSET);
    }

    /// The rows of a focused 30x6 panel with a document `lines` long
    /// drawn from line `first`.
    fn document(lines: usize, first: usize) -> Vec<String> {
        let theme = Theme::cyber();
        let mut terminal = Terminal::new(TestBackend::new(30, 6)).expect("test terminal");
        terminal
            .draw(|frame| {
                let panel = frame.area();
                let inner = render_body_block(frame, panel, (true, false), &theme).unwrap();
                let body = Body::new(panel, inner, true);
                let lines = (0..lines)
                    .map(|n| Line::from(format!("line {n}")))
                    .collect();
                body.draw_document(frame, lines, first, &theme);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        (0..6)
            .map(|y| (0..30).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    fn right_border(rows: &[String]) -> String {
        rows[2..5]
            .iter()
            .filter_map(|row| row.chars().last())
            .collect()
    }

    #[test]
    fn a_document_sits_inside_the_padding_and_scrolls_when_longer_than_its_body() {
        let rows = document(10, 2);
        assert_eq!(rows[1].trim_matches('│').trim(), "", "{rows:?}");
        assert!(rows[2].starts_with("│  line 2"), "{rows:?}");
        assert!(rows[4].starts_with("│  line 4"), "no hint row: {rows:?}");
        assert!(right_border(&rows).contains('┃'), "{rows:?}");

        let rows = document(2, 0);
        assert_eq!(right_border(&rows), "│││", "a document that fits: {rows:?}");
    }
}
