//! Dragging the chat panel's scrollbar.
//!
//! The renderer records the scrollbar it drew on the chat panel's right
//! border: the conversation's, or that of a record, process or file shown
//! over it.
//! A left press on that column grabs it, and so does one on the blank
//! columns between it and the text, up to [`REACH`] of them: one column
//! is a thin thing to hit, and in the desktop window the column right of
//! it belongs to the divider. Pressed on the thumb's rows, the thumb
//! keeps the row it was taken by and the view stays put; pressed above or
//! below it, the thumb jumps to center on the pointer. Dragging then moves
//! the thumb with the pointer, and the view follows: the thumb's top row
//! in the track maps to the first row on screen in proportion, as ratatui
//! draws it, clamped to the list. At either end of its travel the thumb
//! shows that end of the list, however many rows its last track row
//! stands for, so a chat dragged to the bottom follows new output again.
//! Letting go releases it.
//!
//! Menus and lists drawn over the bottom of the chat (the ATTACHED images,
//! `/todos`, the slash and `@` menus) cover the bottom of the track; the
//! rows they cover are theirs to press, not the scrollbar's.

use super::{App, DetailView};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// Most blank columns left of a scrollbar's track that take hold of it
/// too.
pub(crate) const REACH: u16 = 2;

/// What a scrollbar scrolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScrollTarget {
    /// The conversation or terminal in the chat panel
    Chat,
    /// A record, process or file shown over the chat panel
    Detail,
}

/// A scrollbar as drawn in the last frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScrollbarArea {
    pub target: ScrollTarget,
    /// The track: one column of the panel's right border
    pub track: Rect,
    /// Rows in the whole list
    pub total: usize,
    /// Rows that fit on screen
    pub visible: usize,
    /// First row on screen
    pub first: usize,
    /// Largest first row the view scrolls to: for a file whose lines wrap
    /// it can be past `total - visible`
    pub last: usize,
    /// Rows of the track, from its top, that nothing was drawn over
    pub open: u16,
    /// Blank columns between the text and the track
    pub margin: u16,
}

impl ScrollbarArea {
    /// The thumb's first row in the track and its length, as ratatui's
    /// `Scrollbar` places it for these positions.
    fn thumb(&self) -> (usize, usize) {
        let track = usize::from(self.track.height);
        let total = self.total.max(1);
        let rounded = |rows: usize| (rows.saturating_mul(track) + total / 2) / total;
        let first = self.first.min(self.total.saturating_sub(self.visible));
        let start = rounded(first).min(track.saturating_sub(1));
        let end = rounded(first + self.visible).min(track);
        (start, end.saturating_sub(start).max(1))
    }

    /// The thumb's first row in the track with the end of the list on
    /// screen: as low as it goes.
    fn lowest_top(&self) -> usize {
        Self {
            first: self.last,
            ..*self
        }
        .thumb()
        .0
    }

    /// First row to show with the thumb's top on `top` rows down the
    /// track, the inverse of [`Self::thumb`]. At either end of its travel
    /// the thumb shows that end of the list: in a long list a track row
    /// stands for many rows, and mapping the last one back would stop
    /// short of the end.
    fn first_at(&self, top: usize) -> usize {
        if top == 0 {
            return 0;
        }
        if top >= self.lowest_top() {
            return self.last;
        }
        let track = usize::from(self.track.height).max(1);
        ((top.saturating_mul(self.total) + track / 2) / track).min(self.last)
    }

    /// Whether a press on the cell at `column`, `row` takes hold of the
    /// scrollbar: on the track or up to [`REACH`] of the blank columns
    /// left of it, on the rows nothing was drawn over. A list drawn over
    /// the bottom of the chat spans the panel, so the rows it covers on
    /// the track are its rows across those columns too.
    const fn covers(&self, column: u16, row: u16) -> bool {
        let track = self.track;
        let reach = if self.margin < REACH {
            self.margin
        } else {
            REACH
        };
        column <= track.x
            && track.x - column <= reach
            && row >= track.y
            && row < track.y + self.open
    }
}

/// A scrollbar held with the left button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScrollbarGrab {
    target: ScrollTarget,
    /// Rows from the thumb's top to the pointer
    anchor: usize,
}

impl App {
    /// Record the scrollbar drawn on the chat panel this frame, or that
    /// none was. Called once it is drawn into `buffer`, whose cells on its
    /// track then tell what is drawn over it later in the frame (see
    /// [`Self::trim_covered_scrollbar`]).
    pub(crate) fn set_chat_scrollbar(&mut self, scrollbar: Option<ScrollbarArea>, buffer: &Buffer) {
        self.scrollbar_cells.clear();
        if let Some(track) = scrollbar.map(|bar| bar.track) {
            let cells = (track.y..track.bottom())
                .map(|y| buffer.cell((track.x, y)).cloned().unwrap_or_default());
            self.scrollbar_cells.extend(cells);
        }
        self.areas.scrollbar = scrollbar;
    }

    /// Leave the rows at the bottom of the scrollbar's track that were
    /// drawn over since it was, by `buffer`'s cells at the end of the
    /// frame, to what covers them.
    pub(crate) fn trim_covered_scrollbar(&mut self, buffer: &Buffer) {
        let Some(bar) = self.areas.scrollbar.as_mut() else {
            return;
        };
        let track = bar.track;
        let open = (track.y..track.bottom())
            .zip(&self.scrollbar_cells)
            .take_while(|&(y, cell)| buffer.cell((track.x, y)) == Some(cell))
            .count();
        bar.open = u16::try_from(open).unwrap_or(u16::MAX);
    }

    /// Whether a scrollbar is held.
    pub(crate) const fn scrollbar_held(&self) -> bool {
        self.scrollbar_grab.is_some()
    }

    /// Grab, drag or let go of the chat panel's scrollbar with the left
    /// button. Returns whether the event was the scrollbar's.
    pub(super) fn scrollbar_mouse(
        &mut self,
        kind: crossterm::event::MouseEventKind,
        column: u16,
        row: u16,
    ) -> bool {
        use crossterm::event::{MouseButton, MouseEventKind};
        match kind {
            MouseEventKind::Down(MouseButton::Left) => self.grab_scrollbar(column, row),
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(grab) = self.scrollbar_grab else {
                    return false;
                };
                self.drag_scrollbar(grab, row);
                true
            }
            MouseEventKind::Up(MouseButton::Left) => self.scrollbar_grab.take().is_some(),
            _ => false,
        }
    }

    /// Take hold of the scrollbar under `column`, `row`, if there is one.
    fn grab_scrollbar(&mut self, column: u16, row: u16) -> bool {
        let Some(bar) = self.areas.scrollbar.filter(|bar| bar.covers(column, row)) else {
            return false;
        };
        let (start, length) = bar.thumb();
        let at = usize::from(row - bar.track.y);
        let on_thumb = (start..start + length).contains(&at);
        let grab = ScrollbarGrab {
            target: bar.target,
            anchor: if on_thumb { at - start } else { length / 2 },
        };
        self.focus = super::PanelFocus::Chat;
        self.clear_selection();
        self.scrollbar_grab = Some(grab);
        if !on_thumb {
            self.drag_scrollbar(grab, row);
        }
        true
    }

    /// Move the held scrollbar's thumb to the pointer on `row`. A
    /// scrollbar that is no longer drawn lets go.
    fn drag_scrollbar(&mut self, grab: ScrollbarGrab, row: u16) {
        let Some(bar) = self.areas.scrollbar.filter(|bar| bar.target == grab.target) else {
            self.scrollbar_grab = None;
            return;
        };
        let top = usize::from(row.saturating_sub(bar.track.y)).saturating_sub(grab.anchor);
        let first = bar.first_at(top);
        // A thumb already on `top` stays put: a track row stands for
        // several list rows, and mapping it back would round them away.
        // At either end of its travel it goes all the way.
        let now = ScrollbarArea {
            first: self.scrollbar_first(bar),
            ..bar
        };
        if now.thumb().0 == top && first != 0 && first != bar.last {
            return;
        }
        match bar.target {
            ScrollTarget::Chat => self.scroll_chat_to_row(bar, first),
            ScrollTarget::Detail => {
                if self.detail.as_ref().is_some_and(DetailView::scrolls) {
                    self.detail_scroll = first;
                    self.detail_scrolled();
                }
            }
        }
    }

    /// First row the next frame shows under `bar`, as scrolled since it
    /// was drawn.
    fn scrollbar_first(&self, bar: ScrollbarArea) -> usize {
        match bar.target {
            ScrollTarget::Chat => bar.last - self.scroll_offset.min(bar.last),
            ScrollTarget::Detail => self.detail_scroll.min(bar.last),
        }
    }

    /// Show the conversation from row `first` of the `bar` last drawn.
    /// The offset counts rows below the screen, as the wheel and the
    /// smooth scroll move it, so the last row keeps following new output.
    fn scroll_chat_to_row(&mut self, bar: ScrollbarArea, first: usize) {
        self.scroll_offset = bar.last.saturating_sub(first);
    }
}

impl DetailView {
    /// Whether the view scrolls by source line under a scrollbar.
    const fn scrolls(&self) -> bool {
        !matches!(self, Self::Graph | Self::Image(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget};

    fn on_track(total: usize, visible: usize, first: usize, track: u16) -> ScrollbarArea {
        ScrollbarArea {
            target: ScrollTarget::Chat,
            track: Rect::new(9, 2, 1, track),
            total,
            visible,
            first,
            last: total - visible,
            open: track,
            margin: 0,
        }
    }

    fn bar(total: usize, visible: usize, first: usize) -> ScrollbarArea {
        on_track(total, visible, first, 10)
    }

    /// The thumb ratatui draws for `bar`, set up as the renderer sets up
    /// the chat's scrollbar: its first row and length.
    fn drawn_thumb(bar: &ScrollbarArea) -> (usize, usize) {
        let area = Rect::new(0, 0, 1, bar.track.height);
        let mut buffer = Buffer::empty(area);
        let mut state = ScrollbarState::new(bar.total - bar.visible + 1)
            .position(bar.first)
            .viewport_content_length(bar.visible);
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("┃")
            .render(area, &mut buffer, &mut state);
        let rows: Vec<usize> = (0..bar.track.height)
            .filter(|&y| buffer[(0, y)].symbol() == "┃")
            .map(usize::from)
            .collect();
        (rows[0], rows.len())
    }

    #[test]
    fn the_thumb_spans_the_visible_share_of_the_track() {
        assert_eq!(bar(100, 10, 0).thumb(), (0, 1));
        assert_eq!(bar(20, 10, 0).thumb(), (0, 5));
        assert_eq!(bar(20, 10, 10).thumb(), (5, 5));
        assert_eq!(bar(1000, 10, 990).thumb(), (9, 1));
    }

    #[test]
    fn the_blank_columns_left_of_the_track_take_hold_of_it_up_to_the_reach() {
        // The track is column 9, rows 2 to 11; the bottom three rows are
        // covered by a list.
        let covered = |margin| ScrollbarArea {
            margin,
            open: 7,
            ..bar(100, 10, 0)
        };
        let columns = |bar: ScrollbarArea, row| -> Vec<u16> {
            (0..=20).filter(|&column| bar.covers(column, row)).collect()
        };
        assert_eq!(columns(covered(0), 2), [9], "the track alone");
        assert_eq!(columns(covered(1), 2), [8, 9], "a column of padding");
        assert_eq!(columns(covered(REACH), 5), [7, 8, 9]);
        assert_eq!(
            columns(covered(12), 8),
            [7, 8, 9],
            "no further than the reach"
        );
        for margin in [0, 1, REACH, 12] {
            let bar = covered(margin);
            assert!(columns(bar, 1).is_empty(), "above the track");
            assert!(columns(bar, 9).is_empty(), "under the list");
            assert!(columns(bar, 12).is_empty(), "below the track");
        }
        let edge = ScrollbarArea {
            track: Rect::new(0, 0, 1, 10),
            margin: REACH,
            ..bar(100, 10, 0)
        };
        assert_eq!(columns(edge, 0), [0], "nothing left of the screen");
    }

    #[test]
    fn the_thumb_is_where_ratatui_draws_it() {
        for track in [4, 10, 23] {
            for visible in 1..=usize::from(track) {
                for total in (visible + 1..=visible + 30).chain([997, 25_000]) {
                    let last = total - visible;
                    let step = (last / 31).max(1);
                    for first in (0..=last).step_by(step).chain([last]) {
                        let bar = on_track(total, visible, first, track);
                        assert_eq!(bar.thumb(), drawn_thumb(&bar), "{bar:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_thumbs_top_maps_back_to_the_first_row_clamped() {
        let cases = [(0, 0), (5, 10), (3, 6), (9, 10), (40, 10)];
        for (top, first) in cases {
            assert_eq!(bar(20, 10, 0).first_at(top), first, "top {top}");
        }
        // A track row stands for two list rows here, so a drawn thumb
        // maps back to within one of where it was.
        for first in 0..=10 {
            let drawn = bar(20, 10, first);
            assert!(drawn.first_at(drawn.thumb().0).abs_diff(first) <= 1);
        }
    }

    #[test]
    fn a_thumb_at_either_end_shows_that_end_of_a_long_list() {
        // Its last track row stands for rows 850 to 990 of this one.
        let long = bar(1000, 10, 0);
        assert_eq!(long.lowest_top(), 9);
        assert_eq!(long.first_at(9), 990, "the end, not row 900");
        assert_eq!(long.first_at(8), 800);
        for track in [3, 10, 37] {
            for visible in [1, 3, usize::from(track)] {
                for total in [visible + 1, 23, 100, 999, 5000, 100_000] {
                    let Some(last) = total.checked_sub(visible).filter(|&last| last > 0) else {
                        continue;
                    };
                    let bar = on_track(total, visible, 0, track);
                    let lowest = bar.lowest_top();
                    assert_eq!(bar.first_at(0), 0, "{bar:?}");
                    assert_eq!(bar.first_at(lowest), last, "{bar:?}");
                    assert_eq!(bar.first_at(usize::from(track) + 5), last, "{bar:?}");
                    let firsts: Vec<usize> = (0..=lowest).map(|top| bar.first_at(top)).collect();
                    assert!(firsts.is_sorted(), "{bar:?}: {firsts:?}");
                    if total < usize::from(track) {
                        // Track rows outnumber list rows, so some share one.
                        continue;
                    }
                    // Every row the thumb rests on maps back to a first
                    // row it is drawn on.
                    for (top, &first) in firsts.iter().enumerate() {
                        let shown = ScrollbarArea { first, ..bar };
                        assert_eq!(shown.thumb().0, top, "{bar:?} from {top}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_file_whose_end_wraps_scrolls_to_its_last_page() {
        // Long lines at the end leave fewer of them on the last page than
        // are on screen now, so its first line is past `total - visible`.
        let file = ScrollbarArea {
            target: ScrollTarget::Detail,
            last: 95,
            ..bar(100, 10, 0)
        };
        assert_eq!(file.first_at(file.lowest_top()), 95);
        assert_eq!(file.first_at(30), 95, "past the track");
    }
}
