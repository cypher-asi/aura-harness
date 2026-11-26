//! The chat panel's GRAPH tab: what the agent on screen and every agent
//! it spawned did, as turns of cards wired together on a canvas that pans
//! (and, drawn by a front end, zooms).
//!
//! The nodes come from the kernel record ([`UiCommand::FlowLoad`], then
//! [`UiCommand::FlowUpsert`] while agents work). This module keeps them,
//! groups them into turns and places those ([`layout`]), folds them, moves
//! the selection and the view; `renderer::flow` draws the part of the
//! canvas that is on screen, or leaves it to the front end
//! ([`App::set_host_canvas`]).
//!
//! [`UiCommand::FlowLoad`]: crate::events::UiCommand::FlowLoad
//! [`UiCommand::FlowUpsert`]: crate::events::UiCommand::FlowUpsert

mod cards;
mod folding;
mod keys;
pub mod layout;
mod loading;
mod modes;
mod mouse;
mod navigation;
mod open;
#[cfg(test)]
mod tests;
pub mod timeline;
mod view;

use super::App;
use crate::events::{FlowLane, FlowNode, UiEvent};
use layout::FlowLayout;
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet};
use timeline::TimelineState;

/// Nodes kept; the oldest go once a flow grows past this.
const MAX_NODES: usize = 20_000;

/// Agents a flow has above which it is big: every agent but the one on
/// screen starts folded to one card. A benchmark is always big.
pub const BIG_FLOW_LANES: usize = 4;

/// Cells kept clear around the selected card when the view follows it.
const ROOM: f32 = 2.0;
/// Columns one turn of the wheel pans sideways.
const WHEEL_COLUMNS: f32 = 8.0;
/// How far the view zooms out and in.
pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 2.0;
/// What one press of `+` or `-` zooms by.
const ZOOM_STEP: f32 = 1.25;

/// How large the canvas is drawn: 1 is a canvas cell to a screen cell.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Zoom(f32);

impl Default for Zoom {
    fn default() -> Self {
        Self(1.0)
    }
}

/// Agents whose folds, selection and view are kept for when their flow is
/// shown again.
const MAX_KEPT: usize = 32;

/// How a flow is shown: by the GRAPH tab or the TIME tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FlowMode {
    /// Turns as cards wired together on a canvas
    #[default]
    Canvas,
    /// An agent a row, its turns as bars over time
    Timeline,
}

/// What the GRAPH tab keeps of an agent's flow while another's is shown.
#[derive(Debug, Default)]
struct Kept {
    turn_folded: HashSet<String>,
    call_folded: HashSet<String>,
    lane_toggled: HashSet<String>,
    big: bool,
    selected: Option<String>,
    follow_tail: bool,
    pan: (f32, f32),
    zoom: Zoom,
    detached: bool,
    timeline: TimelineState,
}

/// Where the canvas was drawn in the last frame, for the mouse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlowView {
    /// The screen cells the canvas is in
    pub area: Rect,
    /// Canvas cell at the top left of `area`. The terminal draws from
    /// whole cells; the desktop window from the fraction too.
    pub pan: (f32, f32),
}

impl FlowView {
    /// The canvas point in the middle of screen cell `(column, row)`,
    /// when the cell is on the canvas.
    #[must_use]
    pub fn canvas_at(&self, column: u16, row: u16) -> Option<(f32, f32)> {
        let area = self.area;
        let inside =
            column >= area.x && column < area.right() && row >= area.y && row < area.bottom();
        inside.then(|| {
            (
                self.pan.0.floor() + f32::from(column - area.x) + 0.5,
                self.pan.1.floor() + f32::from(row - area.y) + 0.5,
            )
        })
    }
}

/// The flow shown by the GRAPH tab.
#[derive(Debug, Default)]
pub struct FlowState {
    /// Whether the GRAPH tab is the chat panel's open tab
    open: bool,
    /// Hex id of the agent the flow is of
    agent_id: String,
    /// Whether that agent's flow has arrived
    loaded: bool,
    lanes: Vec<FlowLane>,
    lane_index: HashMap<String, usize>,
    /// Nodes in arrival order
    nodes: Vec<FlowNode>,
    index: HashMap<String, usize>,
    /// Turns whose subcards are folded away, by the id of their head
    turn_folded: HashSet<String>,
    /// Model calls whose rows are folded away, by node id
    call_folded: HashSet<String>,
    /// Agents folded to one card, or unfolded, against what they start
    /// as, by hex id
    lane_toggled: HashSet<String>,
    /// Whether the flow is big, so its agents start folded
    big: bool,
    /// Canvas or timeline: which of the two tabs shows the flow
    mode: FlowMode,
    /// The canvas was opened from the timeline, so Esc goes back to it
    from_timeline: bool,
    timeline: TimelineState,
    selected: Option<String>,
    /// Whether the selection moves to each new card as it arrives
    follow_tail: bool,
    /// The node `f` last jumped from, so the next `f` goes somewhere else
    came_from: Option<String>,
    /// Canvas cell at the top left of the view; anywhere, on the canvas
    /// or off it
    pan: (f32, f32),
    /// How large the front end draws the canvas; always 1 in a terminal
    zoom: Zoom,
    /// The view was panned off the selection; it stays there until the
    /// selection moves
    detached: bool,
    /// The folds, selection and view of the flows of agents shown before,
    /// the most recent last
    kept: Vec<(String, Kept)>,
    layout: FlowLayout,
    stale: bool,
    /// Rows of the view the layout fits its columns to; 0 before there
    /// is one
    room: usize,
    view: Option<FlowView>,
    /// Whether the front end draws the canvas in the cells of `view`
    hosted: bool,
    /// The parts of a hosted canvas nothing was drawn over this frame
    clip: Vec<Rect>,
    /// Screen cell a drag that pans the view is at
    drag: Option<(u16, u16)>,
    /// Id of the head of the card whose fold button the pointer is over
    fold_hover: Option<String>,
    /// `m` hid the map of the whole canvas
    minimap_hidden: bool,
    /// Where the map was drawn in the last frame, for the mouse
    minimap: Option<MinimapView>,
    /// The button went down on the map and is still down
    minimap_drag: bool,
}

/// Where the map of the whole canvas was drawn: which screen cells, and
/// how many canvas cells each of them stands for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinimapView {
    /// The map's box, its border included
    pub area: Rect,
    /// Canvas columns, and rows, a cell of the map stands for
    pub scale: (f32, f32),
}

impl MinimapView {
    /// The inside of the box, where the canvas is drawn.
    #[must_use]
    pub const fn inner(&self) -> Rect {
        Rect {
            x: self.area.x + 1,
            y: self.area.y + 1,
            width: self.area.width.saturating_sub(2),
            height: self.area.height.saturating_sub(2),
        }
    }

    /// The canvas point screen cell `(column, row)` of the map stands
    /// for, when it is in the box.
    #[must_use]
    pub fn canvas_at(&self, column: u16, row: u16) -> Option<(f32, f32)> {
        let area = self.area;
        let inside =
            column >= area.x && column < area.right() && row >= area.y && row < area.bottom();
        inside.then(|| self.canvas_near(column, row))
    }

    /// The canvas point of the map's cell nearest screen cell
    /// `(column, row)`.
    #[must_use]
    pub fn canvas_near(&self, column: u16, row: u16) -> (f32, f32) {
        let inner = self.inner();
        let at = |cell: u16, start: u16, end: u16, scale: f32| {
            let cell = cell.clamp(start, end.saturating_sub(1).max(start));
            (f32::from(cell - start) + 0.5) * scale
        };
        (
            at(column, inner.x, inner.right(), self.scale.0),
            at(row, inner.y, inner.bottom(), self.scale.1),
        )
    }
}

/// `850ms`, `3.2s`, `4m12s`.
fn format_ms(ms: u64) -> String {
    match ms {
        0..1_000 => format!("{ms}ms"),
        1_000..60_000 => format!("{}.{}s", ms / 1_000, ms % 1_000 / 100),
        _ => format!("{}m{:02}s", ms / 60_000, ms % 60_000 / 1_000),
    }
}

/// `n` cells, as a position on the canvas.
#[allow(clippy::cast_precision_loss)]
const fn cells(n: usize) -> f32 {
    n as f32
}

impl App {
    /// The flow shown by the GRAPH tab.
    #[must_use]
    pub const fn flow(&self) -> &FlowState {
        &self.flow
    }

    pub(crate) fn flow_mut(&mut self) -> &mut FlowState {
        &mut self.flow
    }

    /// Leave the GRAPH tab's canvas to the front end: the cells it
    /// covers are left blank, [`Self::flow_canvas`] says where they are
    /// and [`Self::flow_canvas_clip`] which of them no overlay covers.
    /// The front end draws the canvas from [`Self::flow`], and pans and
    /// clicks it with [`Self::pan_flow`] and [`Self::click_flow_at`].
    pub fn set_host_canvas(&mut self, on: bool) {
        self.host_canvas = on;
    }

    /// Whether the front end draws the GRAPH tab's canvas.
    #[must_use]
    pub const fn host_canvas(&self) -> bool {
        self.host_canvas
    }

    /// The screen cells of the GRAPH tab's canvas in the last frame, when
    /// the front end draws it and the tab is on screen. Its top left cell
    /// shows canvas cell [`FlowState::pan`].
    #[must_use]
    pub fn flow_canvas(&self) -> Option<Rect> {
        self.flow.hosted_area()
    }

    /// The parts of [`Self::flow_canvas`] to draw the canvas in: those
    /// no overlay, menu or selection was drawn over.
    #[must_use]
    pub fn flow_canvas_clip(&self) -> &[Rect] {
        &self.flow.clip
    }

    /// Ask for the flow of the agent on screen, and for it to be kept up
    /// to date.
    pub(super) fn request_flow(&mut self) {
        if self.active_agent_id.is_empty() {
            return;
        }
        self.flow.watch(&self.active_agent_id);
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::OpenFlow(self.active_agent_id.clone()));
        }
    }

    pub(super) fn load_flow(&mut self, agent_id: &str, lanes: Vec<FlowLane>, nodes: Vec<FlowNode>) {
        if agent_id == self.flow.agent_id {
            self.flow.load(lanes, nodes);
        }
    }

    pub(super) fn upsert_flow(
        &mut self,
        agent_id: &str,
        lanes: Vec<FlowLane>,
        nodes: Vec<FlowNode>,
    ) {
        // Updates to a flow that has not loaded would be lost to the load.
        if agent_id == self.flow.agent_id && self.flow.loaded {
            self.flow.upsert(lanes, nodes);
        }
    }
}
