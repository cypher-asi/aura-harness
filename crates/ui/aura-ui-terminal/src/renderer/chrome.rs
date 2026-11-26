//! What every panel is drawn in: its bordered body block, the [`Body`]
//! inside it (the tab's rows, the key hint while focused) and the
//! scrollbar on its right border.

use super::side_list::INFO_HINT;
use crate::components::truncate_line;
use crate::Theme;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
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

/// A panel's body inside its border: the rows its tab draws in, the key
/// hint on the last row while the panel has focus, and the scrollbar on
/// the panel's right border. Lists and info bodies both draw through one,
/// so every tab scrolls and shows its keys the same way.
#[derive(Debug, Clone, Copy)]
pub(super) struct Body {
    /// The panel, whose right border holds the scrollbar
    panel: Rect,
    /// Where the tab's rows go
    rows: Rect,
    /// The hint's row, while the panel has focus and room for it
    hint: Option<Rect>,
    focused: bool,
}

impl Body {
    /// The body `inner` of the panel bordered by `panel`. A focused body
    /// four rows tall or more keeps its last row for the hint.
    pub(super) const fn new(panel: Rect, inner: Rect, focused: bool) -> Self {
        let (rows, hint) = if focused && inner.height >= 4 {
            let rows = Rect {
                height: inner.height - 1,
                ..inner
            };
            let hint = Rect {
                y: inner.y + inner.height - 1,
                height: 1,
                ..inner
            };
            (rows, Some(hint))
        } else {
            (inner, None)
        };
        Self {
            panel,
            rows,
            hint,
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

    /// Draw `lines`, the rows on screen from `view.first` on, then the
    /// scrollbar for `view` and, while the panel has focus, `hint`.
    pub(super) fn draw(
        &self,
        frame: &mut Frame,
        lines: Vec<Line<'_>>,
        view: ScrollView,
        hint: &str,
        theme: &Theme,
    ) {
        frame.render_widget(Paragraph::new(lines), self.rows);
        render_scrollbar(frame, self.panel, self.rows, view, self.focused, theme);
        if let Some(area) = self.hint {
            let hint = truncate_line(hint, usize::from(area.width).saturating_sub(1));
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(" {hint}"),
                    Style::default().fg(theme.colors.muted),
                ))),
                area,
            );
        }
    }

    /// Draw a whole info document, `lines`, from line `first` (which the
    /// tab keeps inside it), with the hint every info body shares.
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
        let shown = lines.into_iter().skip(first).take(view.visible).collect();
        self.draw(frame, shown, view, INFO_HINT, theme);
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
    fn only_a_focused_body_with_room_keeps_a_hint_row() {
        let inner = Rect::new(1, 1, 20, 6);
        let panel = Rect::new(0, 0, 22, 8);
        let focused = Body::new(panel, inner, true);
        assert_eq!(focused.rows(), Rect::new(1, 1, 20, 5));
        assert_eq!(focused.visible(), 5);
        assert_eq!(focused.width(), 20);
        assert_eq!(Body::new(panel, inner, false).rows(), inner);
        let short = Rect::new(1, 1, 20, 3);
        assert_eq!(Body::new(panel, short, true).rows(), short);
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
                let lines = (0..lines).map(|n| Line::from(format!("line {n}"))).collect();
                body.draw_document(frame, lines, first, &theme);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        (0..6)
            .map(|y| (0..30).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    fn right_border(rows: &[String]) -> String {
        rows[1..4]
            .iter()
            .filter_map(|row| row.chars().last())
            .collect()
    }

    #[test]
    fn a_document_longer_than_its_body_shows_a_scrollbar_and_the_hint() {
        let rows = document(10, 2);
        assert!(rows[1].starts_with("│line 2"), "{rows:?}");
        assert!(rows[3].starts_with("│line 4"), "{rows:?}");
        assert!(rows[4].starts_with("│ ↑↓ scroll  ⇧←→ tabs"), "{rows:?}");
        assert!(right_border(&rows).contains('┃'), "{rows:?}");

        let rows = document(2, 0);
        assert_eq!(right_border(&rows), "│││", "a document that fits: {rows:?}");
    }
}
