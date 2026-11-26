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

pub mod layout;
#[cfg(test)]
mod tests;
pub mod timeline;

use super::file_view::FileView;
use super::format::{format_cost, format_tokens};
use super::trace_json::{node_json, NodeLink};
use super::{App, ChatTab, DetailView, NotificationType, PanelFocus};
use crate::events::{FlowKind, FlowLane, FlowNode, TraceNode, TraceNodeStatus, UiEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind};
use layout::{build_layout, CardKind, FlowLayout, Folds, Spot};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::Color;
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

impl FlowState {
    /// Whether the GRAPH tab is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    /// Whether the flow asked for has arrived.
    #[must_use]
    pub const fn loaded(&self) -> bool {
        self.loaded
    }

    /// Hex id of the agent the flow is of.
    #[must_use]
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// The agents in the flow, the one it was asked for first.
    #[must_use]
    pub fn lanes(&self) -> &[FlowLane] {
        &self.lanes
    }

    /// All nodes, in arrival order.
    #[must_use]
    pub fn nodes(&self) -> &[FlowNode] {
        &self.nodes
    }

    /// The selected node.
    #[must_use]
    pub fn selected_node(&self) -> Option<&FlowNode> {
        let index = *self.index.get(self.selected.as_deref()?)?;
        self.nodes.get(index)
    }

    /// The layout as of the last [`Self::ensure_layout`].
    #[must_use]
    pub const fn layout(&self) -> &FlowLayout {
        &self.layout
    }

    /// The selected card, and its row (`None` for its head).
    #[must_use]
    pub fn selected_place(&self) -> Option<(usize, Option<usize>)> {
        let node = *self.index.get(self.selected.as_deref()?)?;
        let card = self.layout.card_of(node)?;
        Some((card, self.layout.row_of(node)))
    }

    /// The selected card.
    #[must_use]
    pub fn selected_card(&self) -> Option<usize> {
        self.selected_place().map(|(card, _)| card)
    }

    /// Canvas cell at the top left of the view.
    #[must_use]
    pub const fn pan(&self) -> (f32, f32) {
        self.pan
    }

    /// How large the canvas is drawn: screen cells to a canvas cell.
    #[must_use]
    pub const fn zoom(&self) -> f32 {
        self.zoom.0
    }

    /// The kind card `card`'s title names: `turn` for a turn's main card
    /// that starts at a prompt, else its head's kind.
    #[must_use]
    pub fn card_kind_label(&self, card: usize) -> &'static str {
        let placed = &self.layout.cards[card];
        let kind = self.nodes[placed.head].kind;
        if placed.kind == CardKind::Agent {
            "agent"
        } else if placed.kind == CardKind::Turn && kind == FlowKind::Prompt {
            "turn"
        } else {
            kind.label()
        }
    }

    /// Display name of the agent with hex id `agent_id`.
    #[must_use]
    pub fn lane_name(&self, agent_id: &str) -> &str {
        self.lane_index
            .get(agent_id)
            .map_or("agent", |&lane| self.lanes[lane].name.as_str())
    }

    /// What card `card`'s title says beside its kind: the agent for a
    /// turn in a spawned agent's lane, how many model calls a turn has or
    /// how many tool calls a folded call hides, and the head's own detail
    /// (model, tokens, time).
    #[must_use]
    pub fn card_facts(&self, card: usize) -> String {
        let placed = &self.layout.cards[card];
        let node = &self.nodes[placed.head];
        let mut facts: Vec<String> = Vec::new();
        if matches!(placed.kind, CardKind::Turn if placed.lane != 0)
            || placed.kind == CardKind::Agent
        {
            facts.push(self.lanes[placed.lane].name.clone());
        }
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        match placed.kind {
            CardKind::Turn if placed.steps > 0 => {
                let calls = plural(placed.steps, "step", "steps");
                facts.push(if placed.folded {
                    format!("+{calls}")
                } else {
                    calls
                });
            }
            CardKind::Call if placed.folded => {
                facts.push(format!("+{}", plural(placed.steps, "tool", "tools")));
            }
            CardKind::Agent => facts.push(plural(placed.steps, "turn", "turns")),
            _ => {}
        }
        // A call's tokens and time are on its stats line; a folded agent
        // has no detail of its own.
        let on_stats_line = (placed.kind == CardKind::Call && placed.stats.is_some())
            || placed.kind == CardKind::Agent;
        if !node.detail.is_empty() && !on_stats_line {
            facts.push(node.detail.clone());
        }
        facts.join(" · ")
    }

    /// Whether the pointer is over card `card`'s fold button.
    #[must_use]
    pub fn fold_hovered(&self, card: usize) -> bool {
        let head = self.layout.cards[card].head;
        self.fold_hover.as_deref() == Some(self.nodes[head].id.as_str())
    }

    /// The pointer is at canvas point `at`, or off the canvas for `None`.
    /// Returns whether that took it onto or off a fold button.
    fn hover(&mut self, at: Option<(f32, f32)>) -> bool {
        let over = at
            .and_then(|(x, y)| self.layout.spot_at(x, y))
            .and_then(|spot| match spot {
                Spot::Title { card, fold: true } => {
                    Some(self.nodes[self.layout.cards[card].head].id.clone())
                }
                _ => None,
            });
        let changed = over != self.fold_hover;
        self.fold_hover = over;
        changed
    }

    /// The stats line of card `card`, when it has one: the tokens in and
    /// out, `↑48.2k ↓516`, and against the right edge its cost and time,
    /// `$0.042 · 3.2s`.
    #[must_use]
    pub fn card_stats(&self, card: usize) -> Option<(String, String)> {
        let usage = self.layout.cards[card].stats?;
        let tokens = format!(
            "↑{} ↓{}",
            format_tokens(usage.input_tokens),
            format_tokens(usage.output_tokens)
        );
        let spent: Vec<String> = usage
            .cost_usd
            .map(format_cost)
            .into_iter()
            .chain(usage.duration_ms.map(format_ms))
            .collect();
        Some((tokens, spent.join(" · ")))
    }

    /// The row the last line of card `card` shows when the card has more
    /// rows than lines: the selected one when it is among those, else
    /// `None` and the line says how many there are.
    #[must_use]
    pub fn last_line_row(&self, card: usize) -> Option<usize> {
        let shown = self.layout.cards[card].shown_rows();
        match self.selected_place() {
            Some((selected, Some(row))) if selected == card && row >= shown => Some(row),
            _ => None,
        }
    }

    /// What card `card`'s bottom border says when its agent reported back
    /// to the agent that asked it from there: `↩ reported to Agent`, or
    /// `↩ failed for Agent`, and whether the work went well.
    #[must_use]
    pub fn card_report(&self, card: usize) -> Option<(String, TraceNodeStatus)> {
        let &(_, node) = self.layout.reports.iter().find(|(from, _)| *from == card)?;
        let to = &self.nodes[node];
        let failed = to.status == TraceNodeStatus::Error
            || (to.kind == FlowKind::Prompt && to.label.contains(" did not finish the task "));
        let parent = self.lane_name(&to.agent_id);
        if failed {
            Some((format!("↩ failed for {parent}"), TraceNodeStatus::Error))
        } else {
            Some((format!("↩ reported to {parent}"), TraceNodeStatus::Ok))
        }
    }

    /// How the `task` call wire `wire` leaves from went, for a wire that
    /// gives an agent its work: pending while the agent works, then done
    /// or failed. `None` for any other wire.
    #[must_use]
    pub fn wire_status(&self, wire: &layout::Wire) -> Option<TraceNodeStatus> {
        if wire.kind != layout::WireKind::Spawn {
            return None;
        }
        let row = wire.row?;
        let node = *self.layout.cards[wire.from].rows.get(row)?;
        let node = &self.nodes[node];
        (node.kind == FlowKind::Tool).then_some(node.status)
    }

    /// What the last line of card `card` says of the rows it leaves out.
    #[must_use]
    pub fn more_label(&self, card: usize) -> String {
        let placed = &self.layout.cards[card];
        format!("⋯ {} more", placed.rows.len() - placed.shown_rows())
    }

    /// Remember where the canvas was drawn, and whether the front end
    /// draws it.
    pub fn set_view(&mut self, view: Option<FlowView>, hosted: bool) {
        self.view = view;
        self.hosted = hosted && view.is_some();
        self.clip.clear();
    }

    /// Once the frame is drawn: which cells of a hosted canvas are still
    /// marked skipped, nothing having been drawn over them. The marks
    /// come off, as ratatui sends no skipped cell to the backend and the
    /// front end's grid would keep what was there before.
    pub(crate) fn take_canvas_marks(&mut self, buffer: &mut Buffer) {
        self.clip.clear();
        let Some(area) = self.hosted_area() else {
            return;
        };
        // Drawn over without being cleared first, a cell keeps its mark.
        let marked = |cell: &mut Cell| {
            let blank = cell.skip
                && cell.symbol() == " "
                && cell.fg == Color::Reset
                && cell.bg == Color::Reset
                && cell.modifier.is_empty();
            cell.set_skip(false);
            blank
        };
        let mut open: Vec<Rect> = Vec::new();
        for y in area.top()..area.bottom() {
            let mut runs: Vec<(u16, u16)> = Vec::new();
            for x in area.left()..area.right() {
                if !marked(&mut buffer[(x, y)]) {
                    continue;
                }
                match runs.last_mut() {
                    Some((start, width)) if *start + *width == x => *width += 1,
                    _ => runs.push((x, 1)),
                }
            }
            // A run under the same run of the row above makes it taller.
            let (kept, closed): (Vec<Rect>, Vec<Rect>) = open
                .into_iter()
                .partition(|rect| rect.bottom() == y && runs.contains(&(rect.x, rect.width)));
            self.clip.extend(closed);
            open = runs
                .into_iter()
                .map(|(x, width)| {
                    kept.iter()
                        .find(|rect| rect.x == x && rect.width == width)
                        .map_or(Rect::new(x, y, width, 1), |rect| Rect {
                            height: rect.height + 1,
                            ..*rect
                        })
                })
                .collect();
        }
        self.clip.extend(open);
        self.clip.sort_by_key(|rect| (rect.y, rect.x));
    }

    /// The screen cells the front end is to draw the canvas in, when it
    /// draws it.
    #[must_use]
    pub fn hosted_area(&self) -> Option<Rect> {
        self.view.filter(|_| self.hosted).map(|view| view.area)
    }

    /// Show the flow of `agent_id` from now on. Another agent's flow is
    /// dropped, though not its folds, selection and view, which come back
    /// with it; the same agent's stays on screen until the new one arrives.
    fn watch(&mut self, agent_id: &str) {
        if self.agent_id == agent_id {
            return;
        }
        let mut kept = std::mem::take(&mut self.kept);
        if !self.agent_id.is_empty() {
            let state = Kept {
                turn_folded: std::mem::take(&mut self.turn_folded),
                call_folded: std::mem::take(&mut self.call_folded),
                lane_toggled: std::mem::take(&mut self.lane_toggled),
                big: self.big,
                selected: self.selected.take(),
                follow_tail: self.follow_tail,
                pan: self.pan,
                zoom: self.zoom,
                detached: self.detached,
                timeline: std::mem::take(&mut self.timeline),
            };
            kept.push((std::mem::take(&mut self.agent_id), state));
        }
        let state = kept
            .iter()
            .position(|(id, _)| id == agent_id)
            .map(|at| kept.remove(at).1)
            .unwrap_or_default();
        if kept.len() > MAX_KEPT {
            kept.drain(..kept.len() - MAX_KEPT);
        }
        *self = Self {
            open: self.open,
            agent_id: agent_id.to_string(),
            turn_folded: state.turn_folded,
            call_folded: state.call_folded,
            lane_toggled: state.lane_toggled,
            big: state.big,
            selected: state.selected,
            follow_tail: state.follow_tail,
            pan: state.pan,
            zoom: state.zoom,
            detached: state.detached,
            mode: self.mode,
            minimap_hidden: self.minimap_hidden,
            timeline: TimelineState {
                stale: true,
                ..state.timeline
            },
            kept,
            ..Self::default()
        };
    }

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

    /// Whether lane `lane` is shown as one card.
    #[must_use]
    pub fn lane_is_folded(&self, lane: usize) -> bool {
        self.lanes
            .get(lane)
            .is_some_and(|it| self.agent_is_folded(&it.agent_id))
    }

    /// Whether the flow is big enough that its agents start folded.
    #[must_use]
    pub const fn is_big(&self) -> bool {
        self.big
    }

    /// What agent `agent` starts as: folded in a big flow, but for the
    /// agent on screen.
    fn folds_by_default(&self, agent: &str) -> bool {
        self.big && agent != self.agent_id
    }

    fn agent_is_folded(&self, agent: &str) -> bool {
        self.folds_by_default(agent) != self.lane_toggled.contains(agent)
    }

    fn set_agent_folded(&mut self, agent: &str, folded: bool) {
        if folded == self.folds_by_default(agent) {
            self.lane_toggled.remove(agent);
        } else {
            self.lane_toggled.insert(agent.to_string());
        }
    }

    /// The flow is big once it has many agents, or is of a benchmark.
    /// Agents that were on screen when it turned big stay as they were;
    /// the first `known` lanes are those.
    fn update_big(&mut self, known: usize) {
        let big =
            self.lanes.len() > BIG_FLOW_LANES || self.lanes.iter().any(|lane| lane.bench.is_some());
        if big == self.big {
            return;
        }
        let shown: Vec<(String, bool)> = self.lanes[..known.min(self.lanes.len())]
            .iter()
            .map(|lane| (lane.agent_id.clone(), self.agent_is_folded(&lane.agent_id)))
            .collect();
        self.big = big;
        for (agent, folded) in shown {
            self.set_agent_folded(&agent, folded);
        }
    }

    /// Show the flow as `mode`, as the GRAPH or the TIME tab.
    pub(super) fn set_mode(&mut self, mode: FlowMode) {
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
    pub(super) fn restore_mode(&mut self, mode: FlowMode) {
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
    fn timeline_to_selection(&mut self) {
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

    fn load(&mut self, lanes: Vec<FlowLane>, nodes: Vec<FlowNode>) {
        self.lanes.clear();
        self.lane_index.clear();
        self.nodes.clear();
        self.index.clear();
        self.loaded = true;
        self.upsert(lanes, nodes);
        let index = &self.index;
        self.turn_folded.retain(|id| index.contains_key(id));
        self.call_folded.retain(|id| index.contains_key(id));
        let lanes = &self.lane_index;
        self.lane_toggled.retain(|id| lanes.contains_key(id));
    }

    fn upsert(&mut self, lanes: Vec<FlowLane>, nodes: Vec<FlowNode>) {
        let known = self.lanes.len();
        for lane in lanes {
            self.add_lane(lane);
        }
        for node in nodes {
            if let Some(&at) = self.index.get(&node.id) {
                self.nodes[at].merge(node);
                continue;
            }
            if !self.lane_index.contains_key(&node.agent_id) {
                // The lane always comes first; name one that did not
                // rather than drop its nodes.
                self.add_lane(FlowLane {
                    agent_id: node.agent_id.clone(),
                    name: format!("agent {}", node.agent_id.get(..8).unwrap_or(&node.agent_id)),
                    ..FlowLane::default()
                });
            }
            self.index.insert(node.id.clone(), self.nodes.len());
            self.nodes.push(node);
        }
        if self.nodes.len() > MAX_NODES {
            let excess = self.nodes.len() - MAX_NODES * 3 / 4;
            self.nodes.drain(..excess);
            self.index = self
                .nodes
                .iter()
                .enumerate()
                .map(|(at, node)| (node.id.clone(), at))
                .collect();
        }
        self.update_big(known);
        self.stale = true;
        self.timeline.stale = true;
    }

    fn add_lane(&mut self, lane: FlowLane) {
        match self.lane_index.get(&lane.agent_id) {
            Some(&at) => self.lanes[at] = lane,
            None => {
                self.lane_index
                    .insert(lane.agent_id.clone(), self.lanes.len());
                self.lanes.push(lane);
            }
        }
        self.stale = true;
        self.timeline.stale = true;
    }

    /// Fit the columns of cards to a view `rows` tall from the next
    /// layout on.
    pub fn set_room(&mut self, rows: usize) {
        if self.room != rows {
            self.room = rows;
            self.stale = true;
        }
    }

    /// Lay the flow out again if it changed, and keep the selection on
    /// something shown: the newest node while following the end, the head
    /// of what was folded over it.
    pub fn ensure_layout(&mut self) {
        if !self.stale {
            return;
        }
        self.stale = false;
        let lanes: HashSet<String> = self
            .lanes
            .iter()
            .map(|lane| &lane.agent_id)
            .filter(|agent| self.agent_is_folded(agent))
            .cloned()
            .collect();
        let folds = Folds {
            turns: &self.turn_folded,
            calls: &self.call_folded,
            lanes: &lanes,
        };
        self.layout = build_layout(
            &self.lane_index,
            &self.nodes,
            &self.index,
            &folds,
            self.room,
        );
        let selected = self
            .selected
            .as_deref()
            .and_then(|id| self.index.get(id).copied());
        let Some(newest) = self.newest() else {
            self.selected = None;
            return;
        };
        let node = match selected {
            Some(node) if !self.follow_tail => match self.layout.card_of(node) {
                Some(_) if self.is_folded_away(node) => self.layout.stand_in(node),
                Some(_) => node,
                None => newest,
            },
            Some(_) => newest,
            None => {
                self.follow_tail = true;
                newest
            }
        };
        self.selected = Some(self.nodes[node].id.clone());
    }

    /// The newest node shown.
    fn newest(&self) -> Option<usize> {
        self.layout.newest()
    }

    /// Cards with a wire to or from the selected one.
    #[must_use]
    pub fn linked_cards(&self) -> HashSet<usize> {
        let Some(selected) = self.selected_card() else {
            return HashSet::new();
        };
        self.layout
            .wires
            .iter()
            .filter_map(|wire| {
                if wire.from == selected {
                    Some(wire.to)
                } else if wire.to == selected {
                    Some(wire.from)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Links of node `id` in both directions: (edge kind, other node id).
    /// Kinds of edges pointing at `id` are prefixed with `←`.
    #[must_use]
    pub fn links_of(&self, id: &str) -> Vec<(String, String)> {
        let mut links: Vec<(String, String)> = self
            .index
            .get(id)
            .map(|&at| self.nodes[at].links.clone())
            .unwrap_or_default();
        for node in &self.nodes {
            for (kind, other) in &node.links {
                if other == id {
                    links.push((format!("←{kind}"), node.id.clone()));
                }
            }
        }
        links.retain(|(_, other)| self.index.contains_key(other));
        links
    }

    /// Keep the selection inside a view `width` × `height` screen cells,
    /// and that view on the canvas, unless the view was panned off it:
    /// then it stays wherever it was taken. Returns the canvas cell at the
    /// view's top left.
    pub fn sync_view(&mut self, width: f32, height: f32) -> (f32, f32) {
        let (width, height) = (width / self.zoom(), height / self.zoom());
        if self.detached {
            return self.pan;
        }
        if let Some((card, row)) = self.selected_place() {
            let rect = self.layout.card_rect(card);
            let (left, right) = (cells(rect.x) - ROOM, cells(rect.right()) + ROOM);
            if right - left > width || left < self.pan.0 {
                self.pan.0 = left;
            } else if right > self.pan.0 + width {
                self.pan.0 = right - width;
            }
            // The agent's name is on the row above the card.
            let (top, bottom) = (cells(rect.y) - 1.0, cells(rect.bottom()));
            if bottom - top <= height {
                if top < self.pan.1 {
                    self.pan.1 = top;
                } else if bottom > self.pan.1 + height {
                    self.pan.1 = bottom - height;
                }
            } else {
                let line = cells(rect.y + self.layout.cards[card].line_of(row));
                if line < self.pan.1 || line + 1.0 > self.pan.1 + height {
                    self.pan.1 = (line + 1.0 - height / 2.0).max(top);
                }
            }
        }
        self.clamp_pan(width, height);
        self.pan
    }

    /// Keep a view `width` × `height` canvas cells on the canvas.
    fn clamp_pan(&mut self, width: f32, height: f32) {
        let most_x = (cells(self.layout.width) - width).max(0.0);
        let most_y = (cells(self.layout.height) - height).max(0.0);
        self.pan.0 = self.pan.0.clamp(0.0, most_x);
        self.pan.1 = self.pan.1.clamp(0.0, most_y);
    }

    /// Move the view so that canvas point `(x, y)` is in its middle, as
    /// far as the canvas goes; it leaves the selection where it is.
    fn center_on(&mut self, (x, y): (f32, f32)) {
        let Some(area) = self.view.map(|view| view.area) else {
            return;
        };
        let width = f32::from(area.width) / self.zoom();
        let height = f32::from(area.height) / self.zoom();
        self.pan = (x - width / 2.0, y - height / 2.0);
        self.clamp_pan(width, height);
        self.detached = true;
        self.move_view();
    }

    /// Whether the map of the whole canvas is to be drawn: unless `m` hid
    /// it, when the canvas is twice as wide or as tall as a view `width`
    /// × `height` screen cells, or more.
    #[must_use]
    pub fn shows_minimap(&self, width: u16, height: u16) -> bool {
        let zoom = self.zoom();
        !self.minimap_hidden
            && (cells(self.layout.width) >= 2.0 * f32::from(width) / zoom
                || cells(self.layout.height) >= 2.0 * f32::from(height) / zoom)
    }

    /// Where the map of the whole canvas was drawn in the last frame.
    #[must_use]
    pub const fn minimap(&self) -> Option<MinimapView> {
        self.minimap
    }

    /// Where the map was drawn this frame, if it was.
    pub(crate) fn set_minimap(&mut self, map: Option<MinimapView>) {
        self.minimap = map;
        if map.is_none() {
            self.minimap_drag = false;
        }
    }

    /// Move the view the mouse maps through to where the view now is.
    fn move_view(&mut self) {
        if let Some(view) = &mut self.view {
            view.pan = self.pan;
        }
    }

    /// Move the view `dx` × `dy` canvas cells over the canvas, or off it
    /// any distance; it leaves the selection where it is.
    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        let pan = (self.pan.0 + dx, self.pan.1 + dy);
        if !(pan.0.is_finite() && pan.1.is_finite()) {
            return;
        }
        self.pan = pan;
        self.detached = true;
        self.move_view();
    }

    /// Zoom by `factor`, keeping the canvas point at `anchor` where it is
    /// on screen: `anchor` in screen cells from the view's top left, or
    /// its middle for `None`. It leaves the selection where it is.
    pub fn zoom_at(&mut self, factor: f32, anchor: Option<(f32, f32)>) {
        let before = self.zoom();
        let after = (before * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        if !after.is_finite() || after == before {
            return;
        }
        let (ax, ay) = anchor
            .or_else(|| {
                let area = self.view?.area;
                Some((f32::from(area.width) / 2.0, f32::from(area.height) / 2.0))
            })
            .unwrap_or((0.0, 0.0));
        self.pan.0 += ax / before - ax / after;
        self.pan.1 += ay / before - ay / after;
        self.zoom = Zoom(after);
        self.detached = true;
        self.move_view();
    }

    fn select_node(&mut self, node: usize) {
        self.selected = Some(self.nodes[node].id.clone());
        self.follow_tail = self.newest() == Some(node);
        self.detached = false;
        self.came_from = None;
    }

    /// Select row `row` of card `card`, or its head for `None`.
    fn select_place(&mut self, card: usize, row: Option<usize>) {
        let Some(placed) = self.layout.cards.get(card) else {
            return;
        };
        let node = row
            .and_then(|row| placed.rows.get(row).copied())
            .unwrap_or(placed.head);
        self.select_node(node);
    }

    fn select_first(&mut self) {
        if !self.layout.cards.is_empty() {
            self.select_place(0, None);
        }
    }

    fn select_newest(&mut self) {
        if let Some(node) = self.newest() {
            self.select_node(node);
        }
    }

    /// Move to the turn before or after the selected card's, in its
    /// agent's lane.
    fn move_along(&mut self, forward: bool) {
        let Some(card) = self.selected_card() else {
            return self.select_first();
        };
        let main = self.layout.main_of(card);
        let list = self.layout.lane_turns(self.layout.cards[card].lane);
        let Some(at) = list.iter().position(|&c| c == main) else {
            return;
        };
        let next = if forward {
            list.get(at + 1)
        } else {
            at.checked_sub(1).and_then(|at| list.get(at))
        };
        if let Some(&next) = next {
            self.select_place(next, None);
        }
    }

    /// Move a line down or up the selected card, and past its end to the
    /// card under or over it in its column, else the nearest card of the
    /// band below or above.
    fn move_line(&mut self, down: bool) {
        let Some((card, row)) = self.selected_place() else {
            return self.select_first();
        };
        let rows = self.layout.cards[card].rows.len();
        let inside = match (down, row) {
            (true, None) if rows > 0 => Some(Some(0)),
            (true, Some(row)) if row + 1 < rows => Some(Some(row + 1)),
            (false, Some(0)) => Some(None),
            (false, Some(row)) => Some(Some(row - 1)),
            _ => None,
        };
        if let Some(row) = inside {
            return self.select_place(card, row);
        }
        let here = &self.layout.cards[card];
        let (band, column, top) = (here.band, here.column, here.top);
        let target = self
            .layout
            .cards
            .iter()
            .enumerate()
            .filter_map(|(index, other)| {
                let (bands, rows) = if down {
                    (other.band.checked_sub(band)?, other.top)
                } else {
                    // Up into a band, onto the bottom of a column.
                    (band.checked_sub(other.band)?, usize::MAX - other.top)
                };
                let along = other.column.abs_diff(column);
                let rows = match bands {
                    0 if along > 0 => return None,
                    0 if down => other.top.checked_sub(top + 1)?,
                    0 => top.checked_sub(other.top + 1)?,
                    _ => rows,
                };
                Some((bands, along, rows, other.column, index))
            })
            .min();
        if let Some((_, _, _, _, index)) = target {
            let last = self.layout.cards[index].rows.len().checked_sub(1);
            self.select_place(index, if down { None } else { last });
        }
    }

    /// Move to the previous or next prompt, in time order.
    fn move_to_prompt(&mut self, forward: bool) {
        let current = self.selected_card().unwrap_or(0);
        let cards = &self.layout.cards;
        let is_prompt = |index: usize| self.nodes[cards[index].head].kind == FlowKind::Prompt;
        let target = if forward {
            (current + 1..cards.len()).find(|&index| is_prompt(index))
        } else {
            (0..current).rev().find(|&index| is_prompt(index))
        };
        if let Some(index) = target {
            self.select_place(index, None);
        }
    }

    /// Fold or unfold what the selection is in: the rows of a model call,
    /// else its turn. The selection goes to the head of what folds.
    fn toggle_fold(&mut self) {
        let Some(card) = self.selected_card() else {
            return;
        };
        let placed = &self.layout.cards[card];
        if placed.kind == CardKind::Call && placed.folds() {
            self.toggle_card(card);
        } else {
            self.toggle_card(self.layout.main_of(card));
        }
    }

    /// Fold or unfold card `card`: a turn's subcards, a call's rows, a
    /// folded agent's turns.
    fn toggle_card(&mut self, card: usize) {
        let placed = &self.layout.cards[card];
        if !placed.folds() {
            return;
        }
        if placed.kind == CardKind::Agent {
            let agent = self.lanes[placed.lane].agent_id.clone();
            let node = self.selected_node_index().unwrap_or(placed.head);
            return self.unfold_lane(&agent, node);
        }
        let id = self.nodes[placed.head].id.clone();
        let folded = match placed.kind {
            CardKind::Turn => &mut self.turn_folded,
            _ => &mut self.call_folded,
        };
        if !folded.remove(&id) {
            folded.insert(id.clone());
        }
        self.selected = Some(id);
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
    }

    /// Fold every turn, or unfold them all when none is left open.
    fn toggle_fold_all(&mut self) {
        let turns: Vec<(String, bool)> = self
            .layout
            .cards
            .iter()
            .filter(|card| card.kind == CardKind::Turn && card.folds())
            .map(|card| (self.nodes[card.head].id.clone(), card.folded))
            .collect();
        if turns.iter().all(|(_, folded)| *folded) {
            self.turn_folded.clear();
        } else {
            self.turn_folded.extend(turns.into_iter().map(|(id, _)| id));
        }
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
    }

    fn selected_node_index(&self) -> Option<usize> {
        self.index.get(self.selected.as_deref()?).copied()
    }

    /// Show agent `agent`'s turns again, with node `node` selected.
    fn unfold_lane(&mut self, agent: &str, node: usize) {
        self.set_agent_folded(agent, false);
        self.selected = Some(self.nodes[node].id.clone());
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
    }

    /// Show the agent node `node` is of as one card, or its turns again.
    fn toggle_lane_of(&mut self, node: usize) {
        let agent = self.nodes[node].agent_id.clone();
        if self.agent_is_folded(&agent) {
            return self.unfold_lane(&agent, node);
        }
        self.set_agent_folded(&agent, true);
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
    }

    /// Fold lane `lane`, or unfold it, from its name above its band.
    fn toggle_lane(&mut self, lane: usize) {
        let Some(&head) = self
            .layout
            .lane_turns(lane)
            .first()
            .map(|&card| &self.layout.cards[card].head)
        else {
            return;
        };
        self.toggle_lane_of(head);
        self.selected = Some(self.nodes[head].id.clone());
    }

    /// Show every agent as one card but the selected node's and the
    /// agent that spawned it.
    fn fold_other_lanes(&mut self) {
        let Some(node) = self.selected_node_index() else {
            return;
        };
        self.fold_lanes_but(&self.nodes[node].agent_id.clone());
    }

    fn fold_lanes_but(&mut self, agent: &str) {
        let parent = self
            .lane_index
            .get(agent)
            .and_then(|&lane| self.lanes[lane].parent.clone());
        let agents: Vec<String> = self
            .lanes
            .iter()
            .map(|lane| lane.agent_id.clone())
            .collect();
        for id in agents {
            let folded = id != agent && Some(&id) != parent.as_ref();
            self.set_agent_folded(&id, folded);
        }
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
    }

    /// Show every agent's turns.
    fn unfold_lanes(&mut self) {
        let agents: Vec<String> = self
            .lanes
            .iter()
            .map(|lane| lane.agent_id.clone())
            .collect();
        for id in agents {
            self.set_agent_folded(&id, false);
        }
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
    }

    /// From the timeline to the cards of the turn node `node` starts:
    /// every other agent folded but the one that spawned it.
    fn drill_in(&mut self, node: usize) {
        let agent = self.nodes[node].agent_id.clone();
        self.fold_lanes_but(&agent);
        self.turn_folded.remove(&self.nodes[node].id);
        self.selected = Some(self.nodes[node].id.clone());
        self.set_mode(FlowMode::Canvas);
        self.from_timeline = true;
        self.came_from = None;
    }

    /// Unfold whatever hides node `node`.
    fn unfold_to(&mut self, node: usize) {
        if let Some(agent) = self.nodes.get(node).map(|n| n.agent_id.clone()) {
            self.set_agent_folded(&agent, false);
        }
        let (turn, call) = self.layout.folds_over(node);
        if let Some(turn) = turn {
            self.turn_folded.remove(&self.nodes[turn].id);
        }
        if let Some(call) = call {
            self.call_folded.remove(&self.nodes[call].id);
        }
    }

    /// Jump along a link of the selected node, to a node other than the
    /// one the last jump came from when there is a choice. Returns
    /// whether the node has a link to follow.
    fn follow_link(&mut self) -> bool {
        let Some(id) = self.selected.clone() else {
            return false;
        };
        let mut targets: Vec<String> = Vec::new();
        for (_, other) in self.links_of(&id) {
            if !targets.contains(&other) {
                targets.push(other);
            }
        }
        if targets.len() > 1 {
            targets.retain(|other| self.came_from.as_deref() != Some(other));
        }
        let Some(target) = targets.into_iter().next() else {
            return false;
        };
        if let Some(&node) = self
            .index
            .get(&target)
            .filter(|&&node| self.is_folded_away(node))
        {
            // The node is folded away: open what hides it to land on it.
            self.unfold_to(node);
        }
        self.selected = Some(target);
        self.came_from = Some(id);
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
        true
    }

    /// Whether node `node` is in a folded turn, shown only by its prompt.
    fn is_folded_away(&self, node: usize) -> bool {
        match self.layout.card_of(node) {
            Some(card) => {
                self.layout.row_of(node).is_none() && self.layout.cards[card].head != node
            }
            None => false,
        }
    }

    /// What the screen cell `(column, row)` shows of the canvas.
    fn spot_at_cell(&self, column: u16, row: u16) -> Option<Spot> {
        let (x, y) = self.view?.canvas_at(column, row)?;
        self.layout.spot_at(x, y)
    }

    /// Act on a click on `spot`: fold a turn at its marker, select what
    /// was clicked, and open it when it was selected already. Returns
    /// whether something opens.
    fn click(&mut self, spot: Spot) -> bool {
        let (card, row) = match spot {
            Spot::Title { card, fold: true } => {
                self.toggle_card(card);
                return false;
            }
            Spot::Title { card, .. } | Spot::Head(card) => (card, None),
            Spot::Row(card, row) => (card, Some(row)),
            Spot::Lane(lane) => {
                self.toggle_lane(lane);
                return false;
            }
        };
        if self.selected_place() == Some((card, row)) {
            return true;
        }
        self.select_place(card, row);
        false
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

    /// Move the GRAPH tab's view `dx` × `dy` cells over its canvas.
    pub fn pan_flow(&mut self, dx: f32, dy: f32) {
        self.flow.pan_by(dx, dy);
    }

    /// Zoom the GRAPH tab's canvas by `factor` around `anchor`, in screen
    /// cells from the top left of [`Self::flow_canvas`] (its middle for
    /// `None`). Only a front end that draws the canvas zooms it.
    pub fn zoom_flow(&mut self, factor: f32, anchor: Option<(f32, f32)>) {
        if self.host_canvas {
            self.flow.zoom_at(factor, anchor);
        }
    }

    /// A click at canvas point `(x, y)`, in cells: see
    /// [`Self::click_flow`]. Returns whether it was on a card.
    pub fn click_flow_at(&mut self, x: f32, y: f32) -> bool {
        self.flow.ensure_layout();
        let Some(spot) = self.flow.layout.spot_at(x, y) else {
            return false;
        };
        self.focus = PanelFocus::Chat;
        if self.flow.click(spot) {
            self.open_flow_node();
        }
        true
    }

    /// The pointer is at canvas point `at` of the GRAPH tab, in cells, or
    /// off its canvas for `None`. Returns whether a fold button under it
    /// is to be drawn differently.
    pub fn hover_flow_at(&mut self, at: Option<(f32, f32)>) -> bool {
        self.flow.ensure_layout();
        self.flow.hover(at)
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

    /// The wheel over the GRAPH tab: up and down, or sideways with Shift
    /// held or on a wheel that tilts. It leaves the selection alone.
    pub(super) fn scroll_flow(
        &mut self,
        kind: MouseEventKind,
        modifiers: KeyModifiers,
        lines: usize,
    ) {
        let sideways = modifiers.contains(KeyModifiers::SHIFT);
        if self.flow.mode == FlowMode::Timeline {
            let rows = isize::try_from(lines).unwrap_or(isize::MAX);
            let timeline = &mut self.flow.timeline;
            match kind {
                MouseEventKind::ScrollUp if sideways => timeline.pan(-0.1),
                MouseEventKind::ScrollDown if sideways => timeline.pan(0.1),
                MouseEventKind::ScrollUp => timeline.scroll_by(-rows),
                MouseEventKind::ScrollDown => timeline.scroll_by(rows),
                MouseEventKind::ScrollLeft => timeline.pan(-0.1),
                MouseEventKind::ScrollRight => timeline.pan(0.1),
                _ => {}
            }
            return;
        }
        let lines = cells(lines);
        let (dx, dy) = match kind {
            MouseEventKind::ScrollUp if sideways => (-WHEEL_COLUMNS, 0.0),
            MouseEventKind::ScrollDown if sideways => (WHEEL_COLUMNS, 0.0),
            MouseEventKind::ScrollUp => (0.0, -lines),
            MouseEventKind::ScrollDown => (0.0, lines),
            MouseEventKind::ScrollLeft => (-WHEEL_COLUMNS, 0.0),
            MouseEventKind::ScrollRight => (WHEEL_COLUMNS, 0.0),
            _ => return,
        };
        self.flow.pan_by(dx, dy);
    }

    /// Keys of the GRAPH tab, used while the input line is empty. Returns
    /// whether the key was used.
    pub(super) fn handle_flow_key(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        if self.flow.mode == FlowMode::Timeline {
            return self.handle_timeline_key(key);
        }
        self.flow.ensure_layout();
        let page = self.flow.view.map_or(1.0, |view| {
            f32::from(view.area.height.saturating_sub(2).max(1)) / self.flow.zoom()
        });
        match key.code {
            KeyCode::Esc if self.flow.from_timeline => self.show_chat_tab(ChatTab::Time),
            KeyCode::Esc => self.show_chat_tab(ChatTab::Chat),
            KeyCode::Char('m') => {
                self.flow.minimap_hidden = !self.flow.minimap_hidden;
                self.flow.minimap_drag = false;
            }
            KeyCode::Char('c') => {
                if let Some(node) = self.flow.selected_node_index() {
                    self.flow.toggle_lane_of(node);
                }
            }
            KeyCode::Char('C') => self.flow.fold_other_lanes(),
            KeyCode::Char('x') => self.flow.unfold_lanes(),
            KeyCode::Enter
                if self
                    .flow
                    .selected_card()
                    .is_some_and(|card| self.flow.layout.cards[card].kind == CardKind::Agent) =>
            {
                if let Some(card) = self.flow.selected_card() {
                    self.flow.toggle_card(card);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.flow.move_line(false),
            KeyCode::Down | KeyCode::Char('j') => self.flow.move_line(true),
            KeyCode::Left | KeyCode::Char('h') => self.flow.move_along(false),
            KeyCode::Right | KeyCode::Char('l') => self.flow.move_along(true),
            KeyCode::PageUp => self.flow.pan_by(0.0, -page),
            KeyCode::PageDown => self.flow.pan_by(0.0, page),
            KeyCode::Home | KeyCode::Char('g') => self.flow.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.flow.select_newest(),
            KeyCode::Char('[') => self.flow.move_to_prompt(false),
            KeyCode::Char(']') => self.flow.move_to_prompt(true),
            KeyCode::Char(' ') => self.flow.toggle_fold(),
            KeyCode::Char('z') => self.flow.toggle_fold_all(),
            KeyCode::Enter => self.open_flow_node(),
            KeyCode::Char('f') => {
                if !self.flow.follow_link() {
                    self.notifications.current = Some((
                        "This step has no link to another step".to_string(),
                        NotificationType::Warning,
                    ));
                }
            }
            KeyCode::Char('r') => self.jump_to_flow_record(),
            KeyCode::Char('o') => self.open_flow_agent(),
            KeyCode::Char('+' | '=') if self.host_canvas => self.zoom_flow(ZOOM_STEP, None),
            KeyCode::Char('-') if self.host_canvas => self.zoom_flow(1.0 / ZOOM_STEP, None),
            KeyCode::Char('0') => {
                let back = 1.0 / self.flow.zoom();
                self.zoom_flow(back, None);
                self.flow.detached = false;
            }
            _ => return false,
        }
        true
    }

    /// Keys of the GRAPH tab's timeline.
    fn handle_timeline_key(&mut self, key: KeyEvent) -> bool {
        self.flow.ensure_timeline();
        let timeline = &mut self.flow.timeline;
        let page = timeline.view.map_or(10, |view| {
            isize::try_from(view.rows_shown.max(1)).unwrap_or(10)
        });
        let last = timeline.timeline.rows.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => self.show_chat_tab(ChatTab::Chat),
            KeyCode::Up | KeyCode::Char('k') => timeline.move_row(-1),
            KeyCode::Down | KeyCode::Char('j') => timeline.move_row(1),
            KeyCode::Left | KeyCode::Char('h') => timeline.move_span(false),
            KeyCode::Right | KeyCode::Char('l') => timeline.move_span(true),
            KeyCode::PageUp => timeline.scroll_by(-page),
            KeyCode::PageDown => timeline.scroll_by(page),
            KeyCode::Home | KeyCode::Char('g') => timeline.select_row(0),
            KeyCode::End | KeyCode::Char('G') => timeline.select_row(last),
            KeyCode::Char(' ') => {
                if let Some(row) = timeline.selected_row() {
                    timeline.toggle(row);
                }
            }
            KeyCode::Char('z') => timeline.toggle_all(),
            KeyCode::Char('e') => timeline.toggle_issues(),
            KeyCode::Char('+' | '=') => timeline.zoom_by(Some(timeline::ZOOM_STEP)),
            KeyCode::Char('-') => timeline.zoom_by(Some(1.0 / timeline::ZOOM_STEP)),
            KeyCode::Char('0') => timeline.zoom_by(None),
            KeyCode::Enter => self.open_timeline_selection(),
            KeyCode::Char('o') => {
                let lane = timeline
                    .selected_row()
                    .and_then(|row| timeline.timeline.rows[row].lane);
                if let Some(lane) = lane {
                    let agent = self.flow.lanes[lane].agent_id.clone();
                    self.open_agent(agent);
                }
            }
            _ => return false,
        }
        true
    }

    /// Enter on the timeline: the cards of the selected turn, else of the
    /// row's first; a row without turns folds.
    fn open_timeline_selection(&mut self) {
        let timeline = &self.flow.timeline;
        let Some(row) = timeline.selected_row() else {
            return;
        };
        let it = &timeline.timeline.rows[row];
        let head = timeline
            .selected_span()
            .map(|(row, span)| timeline.timeline.rows[row].spans[span].head)
            .or_else(|| it.spans.first().map(|span| span.head));
        match head {
            Some(head) if it.lane.is_some() => self.flow.drill_in(head),
            _ => self.flow.timeline.toggle(row),
        }
    }

    /// A press on the timeline: a row's fold marker folds it, a row or
    /// turn is selected, and a turn already selected opens.
    fn click_timeline(&mut self, column: u16, row: u16) -> bool {
        self.flow.ensure_timeline();
        let timeline = &mut self.flow.timeline;
        match timeline.spot_at(column, row) {
            Some(Err(row)) => timeline.toggle(row),
            Some(Ok((row, span))) => {
                let again =
                    span.is_some() && timeline.selected_row() == Some(row) && timeline.span == span;
                if again {
                    self.open_timeline_selection();
                } else {
                    timeline.select_span(row, span);
                }
            }
            None => return false,
        }
        true
    }

    /// A press in the GRAPH tab: on a turn's fold marker it folds the
    /// turn, on a card's line it selects it, and on the selected line it
    /// opens it. On empty canvas it starts a drag that pans the view.
    /// Returns whether the press was on the canvas.
    pub(super) fn click_flow(&mut self, column: u16, row: u16) -> bool {
        if self.flow.mode == FlowMode::Timeline {
            return self.click_timeline(column, row);
        }
        self.flow.ensure_layout();
        // The map is drawn in cells over a hosted canvas too.
        if let Some(at) = self.flow.minimap.and_then(|map| map.canvas_at(column, row)) {
            self.flow.center_on(at);
            self.flow.minimap_drag = true;
            return true;
        }
        if self.flow.hosted {
            // The front end works its own canvas.
            return false;
        }
        match self.flow.spot_at_cell(column, row) {
            Some(spot) => {
                if self.flow.click(spot) {
                    self.open_flow_node();
                }
                true
            }
            None if self
                .flow
                .view
                .is_some_and(|view| view.canvas_at(column, row).is_some()) =>
            {
                self.flow.drag = Some((column, row));
                true
            }
            None => false,
        }
    }

    /// The pointer moved to `(column, row)` with the button down. Returns
    /// whether a drag of the GRAPH tab's view is under way.
    pub(super) fn drag_flow(&mut self, column: u16, row: u16) -> bool {
        if self.flow.minimap_drag {
            if let Some(map) = self.flow.minimap {
                self.flow.center_on(map.canvas_near(column, row));
            }
            return true;
        }
        let Some((from_column, from_row)) = self.flow.drag else {
            return false;
        };
        let dx = f32::from(from_column) - f32::from(column);
        let dy = f32::from(from_row) - f32::from(row);
        self.flow.pan_by(dx, dy);
        self.flow.drag = Some((column, row));
        true
    }

    /// The button came up. Returns whether it ended a drag of the GRAPH
    /// tab's view.
    pub(super) fn release_flow(&mut self) -> bool {
        std::mem::take(&mut self.flow.minimap_drag) | self.flow.drag.take().is_some()
    }

    /// Show the selected node, with its metadata and links, as JSON over
    /// the chat panel. Closing it comes back to the GRAPH tab.
    fn open_flow_node(&mut self) {
        let Some(node) = self.flow.selected_node() else {
            return;
        };
        let links: Vec<NodeLink> = self
            .flow
            .links_of(&node.id)
            .into_iter()
            .map(|(kind, other)| {
                let label = self
                    .flow
                    .index
                    .get(&other)
                    .map(|&at| self.flow.nodes[at].label.clone());
                (kind, other, label)
            })
            .collect();
        let mut meta = vec![
            (
                "agent".to_string(),
                self.flow.lane_name(&node.agent_id).to_string(),
            ),
            ("agent_id".to_string(), node.agent_id.clone()),
        ];
        let parent = self
            .flow
            .lane_index
            .get(&node.agent_id)
            .and_then(|&lane| self.flow.lanes[lane].parent.as_deref());
        if let Some(parent) = parent {
            meta.push((
                "spawned by".to_string(),
                format!("{} ({parent})", self.flow.lane_name(parent)),
            ));
        }
        if let Some(time) = i64::try_from(node.at_ms)
            .ok()
            .and_then(chrono::DateTime::from_timestamp_millis)
        {
            let local = time.with_timezone(&chrono::Local);
            meta.push((
                "time".to_string(),
                local.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
            ));
        }
        if let Some((card, None)) = self.flow.selected_place() {
            let placed = &self.flow.layout().cards[card];
            if let (CardKind::Turn, Some(usage)) = (placed.kind, placed.stats) {
                let totals = serde_json::json!({
                    "steps": placed.steps,
                    "input_tokens": usage.input_tokens,
                    "output_tokens": usage.output_tokens,
                    "cost_usd": usage.cost_usd,
                    "duration_ms": usage.duration_ms,
                });
                meta.push(("turn".to_string(), totals.to_string()));
            }
        }
        meta.extend(node.meta.iter().cloned());
        let step = TraceNode {
            id: node.id.clone(),
            kind: node.kind.label().to_string(),
            parent: None,
            label: node.label.clone(),
            detail: node.detail.clone(),
            status: node.status,
            seq: Some(node.seq),
            meta,
            links: Vec::new(),
        };
        let title = format!("GRAPH {} {}", step.kind.to_uppercase(), step.label);
        let view = FileView::snapshot(&title, &node_json(&step, &links), "json", 1);
        self.open_detail(DetailView::File(Box::new(view)));
    }

    /// Show the kernel record the selected node comes from in the Record
    /// panel, which lists the agent on screen only.
    fn jump_to_flow_record(&mut self) {
        let Some(node) = self.flow.selected_node() else {
            return;
        };
        let (seq, agent_id) = (node.seq, node.agent_id.clone());
        let found = (agent_id == self.active_agent_id)
            .then(|| self.records.iter().position(|record| record.seq == seq))
            .flatten();
        if let Some(index) = found {
            self.selected_record = index;
            self.records_tab = super::RecordsTab::Kernel;
            self.focus = PanelFocus::Records;
            self.open_detail(DetailView::Record);
            return;
        }
        let message = if agent_id == self.active_agent_id {
            format!("Record #{seq} is no longer in the Record panel")
        } else {
            format!(
                "Record #{seq} is in the record of {}; press o to open that agent",
                self.flow.lane_name(&agent_id)
            )
        };
        self.notifications.current = Some((message, NotificationType::Warning));
    }

    /// Switch to the agent whose lane the selected node is in. It comes
    /// back on the chat panel's tab it was left on, CHAT the first time it
    /// is shown.
    fn open_flow_agent(&mut self) {
        if let Some(agent_id) = self.flow.selected_node().map(|node| node.agent_id.clone()) {
            self.open_agent(agent_id);
        }
    }

    /// Switch to agent `agent_id`, when it has a chat to open.
    fn open_agent(&mut self, agent_id: String) {
        let name = self.flow.lane_name(&agent_id).to_string();
        if agent_id == self.active_agent_id {
            self.notifications.current =
                Some((format!("{name} is already open"), NotificationType::Success));
        } else if self.agents.iter().any(|agent| agent.id == agent_id) {
            self.request_agent_switch(agent_id);
        } else {
            self.notifications.current = Some((
                format!("{name} is a background agent with no chat to open"),
                NotificationType::Warning,
            ));
        }
    }
}
