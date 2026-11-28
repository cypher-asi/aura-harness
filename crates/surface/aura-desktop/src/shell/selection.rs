//! Keeping what the tabs had selected in a file between runs, as the
//! split of projects over tabs is kept (see `router::selection`).
//!
//! The file is read once when the window opens and written from the
//! window's thread whenever the selection changed since the last write,
//! like the tabs' file; it is small. A file that can't be read leaves the
//! tabs on their usual first screen, and one that can't be written is
//! logged and tried again on the next change.

use crate::router::Tabs;
use aura_ui_terminal::LastSelection;
use std::path::PathBuf;

/// Where the selection is kept, and what it was when last written.
pub(super) struct SelectionFile {
    /// `None` in the demo, which keeps nothing
    path: Option<PathBuf>,
    saved: LastSelection,
}

impl SelectionFile {
    /// Read the selection kept at `path` and hand it to `tabs`.
    pub(super) fn open(path: Option<PathBuf>, tabs: &mut Tabs) -> Self {
        let saved = path.as_deref().map(LastSelection::load).unwrap_or_default();
        tabs.remember(saved.clone());
        Self { path, saved }
    }

    /// Write `now` when it differs from what was last written.
    pub(super) fn keep(&mut self, now: LastSelection) {
        let Some(path) = &self.path else {
            return;
        };
        if now == self.saved {
            return;
        }
        // A failed write waits for the next change rather than being
        // tried, and logged, every frame.
        if let Err(e) = now.save(path) {
            tracing::warn!(error = %e, path = %path.display(), "could not save the selection");
        }
        self.saved = now;
    }
}
