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

/// Nodes kept; the oldest go once a flow grows past this.
const MAX_NODES: usize = 6000;

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

/// What the GRAPH tab keeps of an agent's flow while another's is shown.
#[derive(Debug, Default)]
struct Kept {
    turn_folded: HashSet<String>,
    call_folded: HashSet<String>,
    selected: Option<String>,
    follow_tail: bool,
    pan: (f32, f32),
    zoom: Zoom,
    detached: bool,
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
        if placed.kind == CardKind::Turn && kind == FlowKind::Prompt {
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
        if placed.kind == CardKind::Turn && placed.lane != 0 {
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
            _ => {}
        }
        // A call's tokens and time are on its stats line.
        let on_stats_line = placed.kind == CardKind::Call && placed.stats.is_some();
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
                selected: self.selected.take(),
                follow_tail: self.follow_tail,
                pan: self.pan,
                zoom: self.zoom,
                detached: self.detached,
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
            selected: state.selected,
            follow_tail: state.follow_tail,
            pan: state.pan,
            zoom: state.zoom,
            detached: state.detached,
            kept,
            ..Self::default()
        };
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
    }

    fn upsert(&mut self, lanes: Vec<FlowLane>, nodes: Vec<FlowNode>) {
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
                    parent: None,
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
        self.stale = true;
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
        let folds = Folds {
            turns: &self.turn_folded,
            calls: &self.call_folded,
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
                Some(card) if self.is_folded_away(node) => self.layout.cards[card].head,
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

    /// Fold or unfold card `card`: a turn's subcards, a call's rows.
    fn toggle_card(&mut self, card: usize) {
        let placed = &self.layout.cards[card];
        if !placed.folds() {
            return;
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

    /// Unfold whatever hides node `node`.
    fn unfold_to(&mut self, node: usize) {
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
        let lines = cells(lines);
        let sideways = modifiers.contains(KeyModifiers::SHIFT);
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
        self.flow.ensure_layout();
        let page = self.flow.view.map_or(1.0, |view| {
            f32::from(view.area.height.saturating_sub(2).max(1)) / self.flow.zoom()
        });
        match key.code {
            KeyCode::Esc => self.show_chat_tab(ChatTab::Chat),
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
                    self.notification = Some((
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

    /// A press in the GRAPH tab: on a turn's fold marker it folds the
    /// turn, on a card's line it selects it, and on the selected line it
    /// opens it. On empty canvas it starts a drag that pans the view.
    /// Returns whether the press was on the canvas.
    pub(super) fn click_flow(&mut self, column: u16, row: u16) -> bool {
        self.flow.ensure_layout();
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
        self.flow.drag.take().is_some()
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
        self.notification = Some((message, NotificationType::Warning));
    }

    /// Switch to the agent whose lane the selected node is in. The GRAPH
    /// tab then shows that agent's flow.
    fn open_flow_agent(&mut self) {
        let Some(agent_id) = self.flow.selected_node().map(|node| node.agent_id.clone()) else {
            return;
        };
        let name = self.flow.lane_name(&agent_id).to_string();
        if agent_id == self.active_agent_id {
            self.notification =
                Some((format!("{name} is already open"), NotificationType::Success));
        } else if self.agents.iter().any(|agent| agent.id == agent_id) {
            self.request_agent_switch(agent_id);
        } else {
            self.notification = Some((
                format!("{name} is a background agent with no chat to open"),
                NotificationType::Warning,
            ));
        }
    }
}
