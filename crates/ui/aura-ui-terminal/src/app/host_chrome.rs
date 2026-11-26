//! What the front end (host) draws or decides itself: the panels' title
//! bars and the GRAPH canvas, the dividers between panels, the host's
//! workspaces and where each agent runs.

use super::{context_menu, host_tab, PaneChrome, PaneSplits};

/// What the front end draws or decides itself.
#[derive(Debug, Default)]
pub(crate) struct HostChrome {
    /// Whether the front end draws the panels' title bars itself
    pub(crate) chrome: bool,
    /// Whether the front end draws the GRAPH tab's canvas itself
    pub(crate) canvas: bool,
    /// What each panel's title bar showed in the last frame, for a front
    /// end that draws them
    pub(crate) pane_chrome: Vec<PaneChrome>,
    /// Where the front end put the dividers between panels
    pub(crate) pane_splits: PaneSplits,
    /// The host's workspaces, which the menu offers to move a project to
    pub(crate) workspaces: context_menu::Workspaces,
    /// Where each agent runs, and the HOST tab
    pub(crate) cloud: host_tab::CloudUi,
}
