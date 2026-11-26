//! What the mouse selected and holds: the text selection and the pointer
//! dragging it, the chat scrollbar's grab, and what waits to be copied.

use ratatui::{buffer::Cell, layout::Position};
use std::time::Instant;

use super::{scrollbar::ScrollbarGrab, Selection};

/// The mouse selection, the scrollbar grab and the pending copy.
#[derive(Debug, Default)]
pub(crate) struct SelectionState {
    /// Text selected with the mouse
    pub(crate) selection: Option<Selection>,
    /// Where the pointer is while the left button is still held for
    /// `selection`
    pub(crate) selecting: Option<Position>,
    /// The chat panel's scrollbar while the left button holds it
    pub(crate) scrollbar_grab: Option<ScrollbarGrab>,
    /// The cells of the chat panel's scrollbar track as it was drawn this
    /// frame, to tell what was drawn over it after
    pub(crate) scrollbar_cells: Vec<Cell>,
    /// Whether `selection` is in the chat panel's body, whose rows scroll
    pub(crate) selection_scrolls: bool,
    /// When the chat last scrolled for a selection dragged past its edge
    pub(crate) selection_scrolled: Option<Instant>,
    /// Whether the selection should be copied after the next frame.
    /// Dropping the selection drops this, not `copy_pending_text`.
    pub(crate) copy_requested: bool,
    /// Text to copy after the next frame instead of the selection
    pub(crate) copy_pending_text: Option<String>,
    /// What that text is, for the notice once it is copied
    pub(crate) copy_label: Option<String>,
}

impl SelectionState {
    /// Drop the selection, the pointer holding it, its scroll timer and its
    /// copy request; text asked for outright stays to be copied.
    pub(crate) fn clear(&mut self) {
        self.selection = None;
        self.selecting = None;
        self.selection_scrolled = None;
        self.copy_requested = false;
    }
}
