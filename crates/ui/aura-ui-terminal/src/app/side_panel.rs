//! The panel below the Record panel: which view it shows, whose items its
//! lists show, the processes it lists and how far each view is scrolled.

use crate::events::ProcessSummary;

use super::{
    list_state::ListState, scroll::InfoScroll, side_scope::AgentFilter, SidePanelTab, SideScope,
};

/// The side panel's tab, scope, process lists and scroll positions.
#[derive(Debug, Default)]
pub(crate) struct SidePanel {
    /// Which view the panel below the Record panel shows
    pub(crate) tab: SidePanelTab,
    /// Whose items the panel's lists show
    pub(crate) scope: SideScope,
    /// Every process, of every agent
    pub(crate) all_processes: Vec<ProcessSummary>,
    /// Agents whose processes the Processes tab lists
    pub(crate) process_filter: AgentFilter,
    /// Rows of the Processes tab: the processes `process_filter` lets
    /// through; the highlighted row, and the first visible row
    pub(crate) processes: ListState<ProcessSummary>,
    /// First visible row of the TRACE tab
    pub(crate) trace_scroll: usize,
    /// First visible row of the TASKS tab
    pub(crate) tasks_scroll: usize,
    /// How far the POLICIES tab is scrolled
    pub(crate) policies_scroll: InfoScroll,
    /// First visible row of the sandbox tree
    pub(crate) sandbox_scroll: usize,
}
