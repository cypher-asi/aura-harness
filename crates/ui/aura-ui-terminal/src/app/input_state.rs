//! What the input box holds: the text and its cursor, the histories it
//! recalls from, its menus, pasted images and scroll.

use super::{images::PendingImage, mentions::MentionMenu, InputView};
use crate::input::InputHistory;

/// The input box's text, cursor, history, menus, images and scroll.
#[derive(Debug, Default)]
pub(crate) struct InputState {
    /// Current input text
    pub(crate) text: String,
    /// Input history for navigation
    pub(crate) history: InputHistory,
    /// Cursor position in input
    pub(crate) cursor_pos: usize,
    /// Commands run at the terminal prompt, recalled there instead of chat
    /// history
    pub(crate) command_history: InputHistory,
    /// Highlighted row in the slash menu
    pub(crate) slash_cursor: usize,
    /// Whether Esc closed the slash menu (until the input is edited)
    pub(crate) slash_dismissed: bool,
    /// `@` file picker shown while an `@path` word is typed
    pub(crate) mentions: MentionMenu,
    /// Images pasted into the input, each under an `[image N]` chip
    pub(crate) images: Vec<PendingImage>,
    /// Whether Ctrl+V asked for the clipboard before the next frame
    pub(crate) paste_requested: bool,
    /// Where the wheel scrolled the input box's text, if it did
    pub(crate) input_view: Option<InputView>,
}

impl InputState {
    /// Reopen the menu at its first row after the input was edited.
    pub(crate) fn reset_slash_menu(&mut self) {
        self.slash_cursor = 0;
        self.slash_dismissed = false;
    }
}
