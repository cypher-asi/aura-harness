//! The CONTEXT panel: what fills the context window of the agent on
//! screen, opened by `/context` or a click on the ring in the footer
//! ([`crate::app::hud::context_usage`]).

use super::{contains, App, Dialog};
use crate::app::hud::context_usage::ContextUsage;
use ratatui::layout::Rect;

impl App {
    /// Open the CONTEXT panel.
    pub(crate) fn open_context_panel(&mut self) {
        self.open_dialog(Dialog::Context);
    }

    /// The context of the agent on screen, part by part, against its
    /// model's window; `None` before anything filled it.
    #[must_use]
    pub(crate) fn context_usage(&self) -> Option<ContextUsage> {
        let window = self.current_model_option().and_then(|m| m.context_window);
        ContextUsage::of(&self.hud, window)
    }

    /// Record where the footer drew the context ring and its share this
    /// frame, `None` when it drew none.
    pub(crate) fn set_context_ring_area(&mut self, area: Option<Rect>) {
        self.areas.context_ring = area;
    }

    /// Where the footer last drew the context ring.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn context_ring_area(&self) -> Option<Rect> {
        self.areas.context_ring
    }

    /// A left press on the context ring opens the panel. Returns whether
    /// it took the press.
    pub(super) fn click_context_ring(&mut self, column: u16, row: u16) -> bool {
        if !contains(self.areas.context_ring, column, row) {
            return false;
        }
        self.open_context_panel();
        true
    }
}
