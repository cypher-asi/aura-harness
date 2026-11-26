//! Which panels are on screen: the ones the user shows or hides, and the
//! chat or side panel filling the room of the others.

/// Which panels are shown, and which fill the room of the others.
#[derive(Debug)]
pub(crate) struct PanelLayout {
    /// Whether the Record panel is visible
    pub(crate) record_visible: bool,
    /// Whether the Swarm panel is visible
    pub(crate) swarm_visible: bool,
    /// Whether the Sandbox panel is visible
    pub(crate) sandbox_visible: bool,
    /// Whether the chat panel fills the screen, the other panels hidden
    pub(crate) chat_expanded: bool,
    /// Whether the side panel fills the right column, over the Record
    /// panel, while it is shown
    pub(crate) side_expanded: bool,
    /// Which of the Record and side panels were shown when the hamburger
    /// button hid the right column, to show again when it comes back
    pub(crate) side_column_hidden: Option<(bool, bool)>,
    /// Whether the hamburger button hid an open channel's panel. The
    /// channel's right column ignores the agent panels' flags, so it
    /// keeps its own.
    pub(crate) channel_side_hidden: bool,
}

impl Default for PanelLayout {
    /// Every panel shown, none expanded.
    fn default() -> Self {
        Self {
            record_visible: true,
            swarm_visible: true,
            sandbox_visible: true,
            chat_expanded: false,
            side_expanded: false,
            side_column_hidden: None,
            channel_side_hidden: false,
        }
    }
}
