//! Scrolling the chat by pixels, for a front end that can draw the chat's
//! body part of a row off the grid. The UI still scrolls by whole rows;
//! the front end slides the body up to a row's height past them, and
//! shows the row below the body, drawn on its own, coming in at the
//! bottom.

use super::{App, AppState, ProseRow};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use std::hash::{DefaultHasher, Hash, Hasher};

/// Where the chat's rows were in the last frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatScroll {
    /// The chat's body inside its border, in cells: what slides
    pub body: Rect,
    /// Rows of the conversation above the body
    pub above: usize,
    /// Rows of the conversation below the body
    pub below: usize,
}

/// The chat as last drawn, for a front end that scrolls it by pixels.
#[derive(Debug, Default)]
pub(crate) struct SmoothScroll {
    on: bool,
    /// Unless something was drawn over the body after the chat
    view: Option<ChatScroll>,
    /// The row below the body, as wide as the screen
    next: Buffer,
    next_prose: Option<ProseRow>,
    /// The body's cells as the chat left them
    print: u64,
}

impl App {
    /// Draw the chat's row below its body as well (see
    /// [`Self::chat_row_below`]), for a front end that scrolls it by
    /// pixels.
    pub fn set_smooth_scroll(&mut self, on: bool) {
        self.smooth.on = on;
    }

    pub(crate) const fn smooth_scroll(&self) -> bool {
        self.smooth.on
    }

    /// Where the chat's rows were in the last frame, when nothing covered
    /// them: no record, flow, menu or dialog over the body, and no
    /// selection in it. Only then is there a chat to scroll by pixels.
    #[must_use]
    pub fn chat_scroll(&self) -> Option<ChatScroll> {
        let plain = self.detail.is_none()
            && !self.flow.is_open()
            && matches!(self.state, AppState::Idle | AppState::Processing);
        self.smooth.view.filter(|_| plain)
    }

    /// The chat's row below its body in the last frame, its cells as wide
    /// as the screen, and its prose.
    #[must_use]
    pub fn chat_row_below(&self) -> (&[Cell], Option<&ProseRow>) {
        (&self.smooth.next.content, self.smooth.next_prose.as_ref())
    }

    /// Scroll the chat `rows` up, or down when negative, as the wheel
    /// does.
    pub fn scroll_chat_rows(&mut self, rows: isize) {
        self.clear_selection();
        let by = rows.unsigned_abs();
        self.scroll_offset = if rows > 0 {
            self.scroll_offset.saturating_add(by)
        } else {
            self.scroll_offset.saturating_sub(by)
        };
    }

    /// Forget the last frame's chat until the chat is drawn again.
    pub(crate) fn clear_chat_scroll(&mut self) {
        self.smooth.view = None;
    }

    /// The chat was drawn `view`, with `next` and `prose` its row below.
    pub(crate) fn set_chat_scroll(
        &mut self,
        view: ChatScroll,
        buffer: &Buffer,
        (next, prose): (Buffer, Option<ProseRow>),
    ) {
        self.smooth.view = Some(view);
        self.smooth.next = next;
        self.smooth.next_prose = prose;
        self.smooth.print = print(buffer, view.body);
    }

    /// Forget the chat's rows when something was drawn over its body.
    pub(crate) fn drop_covered_chat(&mut self, buffer: &Buffer) {
        if let Some(view) = self.smooth.view {
            if print(buffer, view.body) != self.smooth.print {
                self.smooth.view = None;
            }
        }
    }
}

/// What the cells of `area` in `buffer` hold, in short.
fn print(buffer: &Buffer, area: Rect) -> u64 {
    let mut hasher = DefaultHasher::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buffer.cell((x, y)).hash(&mut hasher);
        }
    }
    hasher.finish()
}
