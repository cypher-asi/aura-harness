//! Where the renderer drew each part of the screen this frame, kept so
//! clicks and hover can be mapped back to what is under them.

use super::{App, InputRows, PanelAreas};
use ratatui::layout::Rect;

impl App {
    /// Record where the panels were drawn this frame.
    pub fn set_panel_areas(&mut self, areas: PanelAreas) {
        self.areas = areas;
    }

    /// Record where the chat messages were drawn this frame.
    pub fn set_chat_text_area(&mut self, area: Rect) {
        self.areas.chat_text = Some(area);
    }

    /// Record the rows the chat panel's body scrolls through this frame
    /// and the content row on top of them, or that it shows nothing that
    /// scrolls by rows.
    pub(crate) fn set_chat_rows(&mut self, rows: Option<(Rect, usize)>) {
        self.areas.chat_rows = rows.map(|(area, _)| area);
        self.areas.chat_rows_top = rows.map_or(0, |(_, top)| top);
    }

    /// Record where the input box was drawn this frame.
    pub fn set_input_area(&mut self, area: Rect) {
        self.areas.input = Some(area);
    }

    /// Record where the input box's stop button was drawn this frame, or
    /// that it was not.
    pub fn set_input_stop_area(&mut self, area: Option<Rect>) {
        self.areas.input_stop = area;
    }

    /// Record the columns the input box wrapped its text at this frame,
    /// or that it was not drawn.
    pub(crate) fn set_input_wrap_width(&mut self, width: Option<usize>) {
        self.areas.input_wrap = width;
    }

    /// Record the rows of the input box's text this frame and which of
    /// them it showed, or that it was not drawn.
    pub(crate) fn set_input_rows(&mut self, rows: Option<InputRows>) {
        self.areas.input_rows = rows;
    }

    /// Text row the wheel scrolled the input box to, while the input and
    /// cursor are as they were when it scrolled; `None` once either
    /// changed, so the box follows the cursor again.
    #[must_use]
    pub(crate) fn input_scroll_top(&self) -> Option<usize> {
        self.input_view
            .as_ref()
            .filter(|view| view.cursor == self.cursor_pos && view.input == self.input)
            .map(|view| view.top)
    }

    /// Where the input box's stop button was drawn last frame.
    #[cfg(test)]
    pub(crate) const fn input_stop_area(&self) -> Option<Rect> {
        self.areas.input_stop
    }

    /// Record where the record list rows were drawn this frame.
    pub fn set_record_rows_area(&mut self, area: Rect) {
        self.areas.record_rows = Some(area);
    }

    /// Record where the sandbox tree rows were drawn this frame and which
    /// tree row is at the top.
    pub fn set_sandbox_rows_area(&mut self, area: Rect, scroll: usize) {
        self.areas.sandbox_rows = Some(area);
        self.areas.sandbox_scroll = scroll;
    }

    /// Record where the side panel's expand button was drawn this frame.
    pub fn set_side_expand_area(&mut self, area: Option<Rect>) {
        self.areas.side_expand = area;
    }

    /// Record where the header's hamburger button was drawn this frame,
    /// or that it was not.
    pub fn set_side_toggle_area(&mut self, area: Option<Rect>) {
        self.areas.side_toggle = area;
    }

    /// The header row's cells of the hamburger button that shows or hides
    /// the right column (see [`Self::toggle_side_column`]) in the last
    /// frame. A front end that draws the chrome finds them blank and draws
    /// the button there itself.
    #[must_use]
    pub const fn side_toggle_area(&self) -> Option<Rect> {
        self.areas.side_toggle
    }

    /// Record where the close button of the view over the chat panel was
    /// drawn this frame.
    pub fn set_detail_close_area(&mut self, area: Option<Rect>) {
        self.areas.detail_close = area;
    }

    /// Record where the chat panel's expand button was drawn this frame.
    pub fn set_chat_expand_area(&mut self, area: Option<Rect>) {
        self.areas.chat_expand = area;
    }
}
