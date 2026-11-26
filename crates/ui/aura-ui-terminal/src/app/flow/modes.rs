//! The GRAPH and TIME tabs' modes of showing a flow.

use super::timeline::TimelineState;
use super::{FlowMode, FlowState};

impl FlowState {
    /// How the flow is shown.
    #[must_use]
    pub const fn mode(&self) -> FlowMode {
        self.mode
    }

    /// The timeline and where in it the user is, as of the last
    /// [`Self::ensure_timeline`].
    #[must_use]
    pub const fn timeline(&self) -> &TimelineState {
        &self.timeline
    }

    pub(crate) fn timeline_mut(&mut self) -> &mut TimelineState {
        &mut self.timeline
    }

    /// Build the timeline again if the flow or its folds changed.
    pub fn ensure_timeline(&mut self) {
        self.timeline
            .rebuild(&self.lanes, &self.lane_index, &self.nodes);
    }

    /// Show the flow as `mode`, as the GRAPH or the TIME tab.
    pub(in crate::app) fn set_mode(&mut self, mode: FlowMode) {
        self.restore_mode(mode);
        self.from_timeline = false;
        if mode == FlowMode::Timeline {
            self.timeline_to_selection();
        }
    }

    /// Show the flow as `mode` without moving the timeline's selection to
    /// the cards': for an agent coming back on GRAPH or TIME, whose timeline
    /// and cards are as it left them, and for another agent's flow about to
    /// be put away.
    pub(in crate::app) fn restore_mode(&mut self, mode: FlowMode) {
        self.mode = mode;
        self.drag = None;
        self.minimap_drag = false;
    }

    /// The canvas was opened from the timeline.
    #[must_use]
    pub const fn from_timeline(&self) -> bool {
        self.from_timeline
    }

    /// Select the timeline row of the agent the canvas has selected, and
    /// the turn it is in.
    pub(super) fn timeline_to_selection(&mut self) {
        self.ensure_timeline();
        let Some(&node) = self.selected.as_deref().and_then(|id| self.index.get(id)) else {
            return;
        };
        let agent = self.nodes[node].agent_id.clone();
        let Some(row) = self.timeline.timeline.row_of(&agent) else {
            return;
        };
        let head = self.layout.folds_over(node).0;
        let span = self.timeline.timeline.rows[row]
            .spans
            .iter()
            .position(|span| Some(span.head) == head || span.head == node);
        self.timeline.select_span(row, span);
    }
}
