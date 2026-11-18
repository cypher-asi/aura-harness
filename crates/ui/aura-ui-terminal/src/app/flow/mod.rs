//! The chat panel's GRAPH tab: what the agent on screen and every agent
//! it spawned did, as cards wired together on a canvas that pans.
//!
//! The nodes come from the kernel record ([`UiCommand::FlowLoad`], then
//! [`UiCommand::FlowUpsert`] while agents work). This module keeps them,
//! groups them into cards and places those ([`layout`]), moves the
//! selection and the view; `renderer::flow` draws the part of the canvas
//! that is on screen, or leaves it to the front end
//! ([`App::set_host_canvas`]).
//!
//! [`UiCommand::FlowLoad`]: crate::events::UiCommand::FlowLoad
//! [`UiCommand::FlowUpsert`]: crate::events::UiCommand::FlowUpsert

pub mod layout;
#[cfg(test)]
mod tests;

use super::file_view::FileView;
use super::trace_json::{node_json, NodeLink};
use super::{App, ChatTab, DetailView, NotificationType, PanelFocus};
use crate::events::{FlowKind, FlowLane, FlowNode, TraceNode, UiEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind};
use layout::{build_layout, FlowLayout, Spot};
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
    /// Prompts whose turn is folded, by node id
    collapsed: HashSet<String>,
    selected: Option<String>,
    /// Whether the selection moves to each new card as it arrives
    follow_tail: bool,
    /// The node `f` last jumped from, so the next `f` goes somewhere else
    came_from: Option<String>,
    /// Canvas cell at the top left of the view
    pan: (f32, f32),
    /// The view was panned off the selection; it stays there until the
    /// selection moves
    detached: bool,
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

    /// Display name of the agent with hex id `agent_id`.
    #[must_use]
    pub fn lane_name(&self, agent_id: &str) -> &str {
        self.lane_index
            .get(agent_id)
            .map_or("agent", |&lane| self.lanes[lane].name.as_str())
    }

    /// What card `card`'s title says beside its kind: the agent for a
    /// prompt in a spawned agent's lane, how many cards a folded turn
    /// hides, and the head's own detail (model, tokens, time).
    #[must_use]
    pub fn card_facts(&self, card: usize) -> String {
        let placed = &self.layout.cards[card];
        let node = &self.nodes[placed.head];
        let mut facts: Vec<String> = Vec::new();
        if node.kind == FlowKind::Prompt && placed.lane != 0 {
            facts.push(self.lanes[placed.lane].name.clone());
        }
        if placed.folded {
            let unit = if placed.steps == 1 { "step" } else { "steps" };
            facts.push(format!("+{} {unit}", placed.steps));
        }
        if !node.detail.is_empty() {
            facts.push(node.detail.clone());
        }
        facts.join(" · ")
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
    /// dropped; the same agent's stays on screen until the new one arrives.
    fn watch(&mut self, agent_id: &str) {
        if self.agent_id == agent_id {
            return;
        }
        let open = self.open;
        *self = Self {
            open,
            agent_id: agent_id.to_string(),
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
        self.collapsed.retain(|id| index.contains_key(id));
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
    /// something shown: the newest card while following the end, the
    /// prompt of a turn that was folded over it.
    pub fn ensure_layout(&mut self) {
        if !self.stale {
            return;
        }
        self.stale = false;
        self.layout = build_layout(
            &self.lane_index,
            &self.nodes,
            &self.index,
            &self.collapsed,
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
            Some(node) if !self.follow_tail => {
                if self.is_folded_away(node) {
                    self.layout.turn_of(node).unwrap_or(newest)
                } else if self.layout.card_of(node).is_some() {
                    node
                } else {
                    newest
                }
            }
            Some(_) => newest,
            None => {
                self.follow_tail = true;
                newest
            }
        };
        self.selected = Some(self.nodes[node].id.clone());
    }

    /// The node the newest card ends with.
    fn newest(&self) -> Option<usize> {
        let card = self.layout.cards.last()?;
        Some(card.rows.last().copied().unwrap_or(card.head))
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

    /// Keep the selection inside a view `width` × `height` cells, unless
    /// the view was panned off it, and the view on the canvas. Returns the
    /// canvas cell at the view's top left.
    pub fn sync_view(&mut self, width: f32, height: f32) -> (f32, f32) {
        if let Some((card, row)) = self.selected_place().filter(|_| !self.detached) {
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

    fn clamp_pan(&mut self, width: f32, height: f32) {
        let most_x = (cells(self.layout.width) - width).max(0.0);
        let most_y = (cells(self.layout.height) - height).max(0.0);
        self.pan.0 = self.pan.0.clamp(0.0, most_x);
        self.pan.1 = self.pan.1.clamp(0.0, most_y);
    }

    /// Move the view `dx` × `dy` cells over the canvas; it leaves the
    /// selection where it is.
    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        if !(dx.is_finite() && dy.is_finite()) {
            return;
        }
        self.pan.0 += dx;
        self.pan.1 += dy;
        self.detached = true;
        if let Some(view) = self.view {
            self.clamp_pan(f32::from(view.area.width), f32::from(view.area.height));
            self.view = Some(FlowView {
                pan: self.pan,
                ..view
            });
        }
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

    /// Move to the card before or after the selected one in its agent's
    /// lane.
    fn move_along(&mut self, forward: bool) {
        let Some(card) = self.selected_card() else {
            return self.select_first();
        };
        let list = self.layout.lane_cards(self.layout.cards[card].lane);
        let Some(at) = list.iter().position(|&c| c == card) else {
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

    /// Fold or unfold the turn the selected card is in, leaving the
    /// selection on its prompt.
    fn toggle_fold(&mut self) {
        let Some(card) = self.selected_card() else {
            return;
        };
        let head = self.layout.cards[card].head;
        let prompt = self.layout.turn_of(head).unwrap_or(head);
        self.toggle_fold_of(prompt);
    }

    fn toggle_fold_of(&mut self, prompt: usize) {
        let has_steps = self
            .layout
            .card_of(prompt)
            .is_some_and(|card| self.layout.cards[card].steps > 0);
        if !has_steps {
            return;
        }
        let id = self.nodes[prompt].id.clone();
        if !self.collapsed.remove(&id) {
            self.collapsed.insert(id.clone());
        }
        self.selected = Some(id);
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
    }

    /// Fold every turn, or unfold them all when none is left open.
    fn toggle_fold_all(&mut self) {
        let turns: Vec<&str> = self
            .layout
            .cards
            .iter()
            .filter(|card| card.steps > 0)
            .map(|card| self.nodes[card.head].id.as_str())
            .collect();
        if turns.iter().all(|id| self.collapsed.contains(*id)) {
            self.collapsed.clear();
        } else {
            self.collapsed = turns.into_iter().map(str::to_string).collect();
        }
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
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
        if let Some(prompt) = self
            .index
            .get(&target)
            .filter(|&&node| self.is_folded_away(node))
            .and_then(|&node| self.layout.turn_of(node))
        {
            // The node is folded away: open its turn to land on it.
            self.collapsed.remove(&self.nodes[prompt].id);
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
                let prompt = self.layout.cards[card].head;
                self.toggle_fold_of(prompt);
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
            f32::from(view.area.height.saturating_sub(2).max(1))
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
        if let Some(time) = i64::try_from(node.at_ms)
            .ok()
            .and_then(chrono::DateTime::from_timestamp_millis)
        {
            let local = time.with_timezone(&chrono::Local);
            meta.push((
                "time".to_string(),
                local.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
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
