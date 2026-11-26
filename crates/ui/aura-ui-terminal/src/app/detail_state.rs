//! The record or file shown over the chat panel, and how far it is
//! scrolled.

use super::DetailView;

/// The detail view over the chat panel and its scroll.
#[derive(Debug)]
pub(crate) struct DetailState {
    /// Record or file shown over the chat panel
    pub(crate) view: Option<DetailView>,
    /// First line of the detail view shown
    pub(crate) scroll: usize,
    /// Source lines on screen in the detail view in the last frame
    pub(crate) shown: usize,
}

impl Default for DetailState {
    /// Nothing shown, at the top, one line on screen.
    fn default() -> Self {
        Self {
            view: None,
            scroll: 0,
            shown: 1,
        }
    }
}
