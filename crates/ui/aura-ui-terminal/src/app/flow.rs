//! The chat panel's GRAPH tab: what the agent on screen and every agent
//! it spawned did, as boxes in time order with one column per agent.
//!
//! The nodes come from the kernel record ([`UiCommand::FlowLoad`], then
//! [`UiCommand::FlowUpsert`] while agents work). This module keeps them,
//! lays them out and moves the selection; `renderer::flow` draws the part
//! of the layout that is on screen.
//!
//! Every box has two rows of its own, its title and its label, and its
//! bottom border on the title row of the box below. Boxes that follow
//! each other in one agent's lane therefore share a border and read as
//! one stack, and a row never holds the label of more than one box, which
//! leaves it free for the arrow into that box.
//!
//! [`UiCommand::FlowLoad`]: crate::events::UiCommand::FlowLoad
//! [`UiCommand::FlowUpsert`]: crate::events::UiCommand::FlowUpsert

use super::file_view::FileView;
use super::trace_json::{node_json, NodeLink};
use super::{follow_selection, App, ChatTab, DetailView, NotificationType, PanelFocus};
use crate::events::{FlowKind, FlowLane, FlowNode, TraceNode, UiEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet};

/// Nodes kept; the oldest go once a flow grows past this.
const MAX_NODES: usize = 6000;

/// Screen rows a box does not share with the box below it.
pub(crate) const BOX_ROWS: usize = 2;

/// Narrowest and widest a lane is drawn, borders and margins included.
const MIN_LANE_WIDTH: u16 = 24;
const MAX_LANE_WIDTH: u16 = 64;

/// Stands for "none" where a box or node index is expected.
const NO_BOX: usize = usize::MAX;

/// One box of the layout, in the order the boxes are drawn top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowBox {
    /// Index into [`FlowState::nodes`]
    pub node: usize,
    /// Index into [`FlowState::lanes`]
    pub lane: usize,
    /// Column its lane is drawn in
    pub column: usize,
    /// Steps of the turn this prompt started; 0 for every other box
    pub steps: usize,
    /// Whether those steps are folded into this box
    pub folded: bool,
}

/// Where one agent's lane runs: its column, and the boxes it spans. The
/// span reaches past the lane's own boxes to the boxes its arrows go to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneSpan {
    /// Index into [`FlowState::lanes`]
    pub lane: usize,
    pub column: usize,
    /// First and last box index
    pub first: usize,
    pub last: usize,
}

/// An arrow between two boxes in different lanes, by box index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowEdge {
    pub from: usize,
    pub to: usize,
}

/// Where every box of the flow goes.
#[derive(Debug, Default)]
pub struct FlowLayout {
    /// Boxes, top to bottom
    pub boxes: Vec<FlowBox>,
    /// Lanes with a box, sorted by column then first box
    pub spans: Vec<LaneSpan>,
    /// Arrows, sorted by the box they point at
    pub edges: Vec<FlowEdge>,
    /// Columns in use
    pub columns: usize,
    /// Box of each node; [`NO_BOX`] for a step folded into its prompt
    box_of: Vec<usize>,
    /// The prompt that started each node's turn; [`NO_BOX`] for prompts
    /// and for steps before a lane's first prompt
    turn_of: Vec<usize>,
}

impl FlowLayout {
    /// Screen rows of the whole flow.
    #[must_use]
    pub fn rows(&self) -> usize {
        if self.boxes.is_empty() {
            0
        } else {
            self.boxes.len() * BOX_ROWS + 1
        }
    }

    /// The arrows pointing at box `index`.
    #[must_use]
    pub fn edges_into(&self, index: usize) -> &[FlowEdge] {
        let start = self.edges.partition_point(|edge| edge.to < index);
        let end = self.edges.partition_point(|edge| edge.to <= index);
        &self.edges[start..end]
    }

    /// The span of the lane box `index` is in.
    #[must_use]
    pub fn span_of(&self, index: usize) -> Option<&LaneSpan> {
        let lane = self.boxes.get(index)?.lane;
        self.spans.iter().find(|span| span.lane == lane)
    }

    fn box_of(&self, node: usize) -> Option<usize> {
        self.box_of.get(node).copied().filter(|&b| b != NO_BOX)
    }

    fn turn_of(&self, node: usize) -> Option<usize> {
        self.turn_of.get(node).copied().filter(|&p| p != NO_BOX)
    }
}

/// Where the flow was drawn in the last frame, for the mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowView {
    /// The rows the boxes are drawn in, below the lane names
    pub rows: Rect,
    /// Flow row at the top of `rows`
    pub first_row: usize,
    /// Column at the left of `rows`
    pub first_column: usize,
    /// Screen columns each lane takes
    pub lane_width: u16,
}

impl FlowView {
    /// How wide each lane is, and how many fit, in `width` screen columns
    /// when the flow has `columns` lanes side by side.
    #[must_use]
    pub fn lanes_in(width: u16, columns: usize) -> (u16, usize) {
        let columns = u16::try_from(columns.max(1)).unwrap_or(u16::MAX);
        let lane_width = (width / columns)
            .clamp(MIN_LANE_WIDTH, MAX_LANE_WIDTH)
            .min(width)
            .max(1);
        (lane_width, usize::from(width / lane_width).max(1))
    }

    /// First screen column and width of a box in lane column `column`,
    /// when that column is on screen.
    #[must_use]
    pub fn box_columns(&self, column: usize) -> Option<(u16, u16)> {
        let offset = u16::try_from(column.checked_sub(self.first_column)?).ok()?;
        let x = self
            .rows
            .x
            .checked_add(offset.checked_mul(self.lane_width)?)?;
        let fits = x.checked_add(self.lane_width)? <= self.rows.x + self.rows.width;
        (fits && self.lane_width >= 6).then(|| (x + 1, self.lane_width - 2))
    }

    /// Screen column of the lifeline of lane column `column`, when that
    /// column is on screen.
    #[must_use]
    pub fn lifeline(&self, column: usize) -> Option<u16> {
        self.box_columns(column).map(|(x, width)| x + width / 2)
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
    /// Whether the selection moves to each new box as it arrives
    follow_tail: bool,
    /// The node `f` last jumped from, so the next `f` goes somewhere else
    came_from: Option<String>,
    scroll: usize,
    /// The wheel moved the view off the selection; it stays there until
    /// the selection moves
    detached: bool,
    first_column: usize,
    layout: FlowLayout,
    stale: bool,
    view: Option<FlowView>,
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

    /// Box of the selected node.
    #[must_use]
    pub fn selected_box(&self) -> Option<usize> {
        let node = *self.index.get(self.selected.as_deref()?)?;
        self.layout.box_of(node)
    }

    /// Display name of the agent with hex id `agent_id`.
    #[must_use]
    pub fn lane_name(&self, agent_id: &str) -> &str {
        self.lane_index
            .get(agent_id)
            .map_or("agent", |&lane| self.lanes[lane].name.as_str())
    }

    /// Remember where the flow was drawn.
    pub fn set_view(&mut self, view: Option<FlowView>) {
        self.view = view;
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

    /// Lay the flow out again if it changed, and keep the selection on a
    /// box: the newest while following the end, the prompt of a turn that
    /// was folded over it.
    pub fn ensure_layout(&mut self) {
        if !self.stale {
            return;
        }
        self.stale = false;
        self.layout = build_layout(&self.lane_index, &self.nodes, &self.index, &self.collapsed);
        let selected = self
            .selected
            .as_deref()
            .and_then(|id| self.index.get(id).copied());
        let Some(last) = self.layout.boxes.last() else {
            self.selected = None;
            return;
        };
        let node = match selected {
            Some(node) if !self.follow_tail => match self.layout.box_of(node) {
                Some(_) => node,
                None => self.layout.turn_of(node).unwrap_or(last.node),
            },
            Some(_) => last.node,
            None => {
                self.follow_tail = true;
                last.node
            }
        };
        self.selected = Some(self.nodes[node].id.clone());
    }

    /// Boxes with an arrow to or from the selected one.
    #[must_use]
    pub fn linked_boxes(&self) -> HashSet<usize> {
        let Some(selected) = self.selected_box() else {
            return HashSet::new();
        };
        self.layout
            .edges
            .iter()
            .filter_map(|edge| {
                if edge.from == selected {
                    Some(edge.to)
                } else if edge.to == selected {
                    Some(edge.from)
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

    /// Keep the selected box inside a view `visible_rows` tall and
    /// `visible_columns` lanes wide, returning the first row and the first
    /// column to draw.
    pub fn sync_scroll(&mut self, visible_rows: usize, visible_columns: usize) -> (usize, usize) {
        let total = self.layout.rows();
        let max_scroll = total.saturating_sub(visible_rows.max(1));
        if let Some(selected) = self.selected_box().filter(|_| !self.detached) {
            let top = selected * BOX_ROWS;
            let bottom = top + BOX_ROWS;
            if top < self.scroll {
                self.scroll = top;
            } else if bottom >= self.scroll + visible_rows {
                self.scroll = (bottom + 1).saturating_sub(visible_rows);
            }
            self.first_column = follow_selection(
                self.first_column,
                self.layout.boxes[selected].column,
                visible_columns,
                self.layout.columns,
            );
        }
        self.scroll = self.scroll.min(max_scroll);
        self.first_column = self
            .first_column
            .min(self.layout.columns.saturating_sub(visible_columns.max(1)));
        (self.scroll, self.first_column)
    }

    fn select_box(&mut self, index: usize) {
        let Some(last) = self.layout.boxes.len().checked_sub(1) else {
            return;
        };
        let index = index.min(last);
        self.selected = Some(self.nodes[self.layout.boxes[index].node].id.clone());
        self.follow_tail = index == last;
        self.detached = false;
        self.came_from = None;
    }

    fn move_by(&mut self, delta: isize) {
        let current = self.selected_box().unwrap_or(0);
        self.select_box(current.saturating_add_signed(delta));
    }

    /// Move to the nearest box in the closest lane to the left or right.
    fn move_across(&mut self, right: bool) {
        let Some(current) = self.selected_box() else {
            return;
        };
        let column = self.layout.boxes[current].column;
        let side = |other: usize| {
            if right {
                other.checked_sub(column)
            } else {
                column.checked_sub(other)
            }
        };
        let target = self
            .layout
            .boxes
            .iter()
            .enumerate()
            .filter_map(|(index, placed)| {
                let away = side(placed.column).filter(|&away| away > 0)?;
                Some((away, index.abs_diff(current), index))
            })
            .min();
        if let Some((_, _, index)) = target {
            self.select_box(index);
        }
    }

    /// Move to the previous or next prompt.
    fn move_to_prompt(&mut self, forward: bool) {
        let current = self.selected_box().unwrap_or(0);
        let is_prompt = |placed: &FlowBox| self.nodes[placed.node].kind == FlowKind::Prompt;
        let boxes = &self.layout.boxes;
        let target = if forward {
            (current + 1..boxes.len()).find(|&index| is_prompt(&boxes[index]))
        } else {
            (0..current).rev().find(|&index| is_prompt(&boxes[index]))
        };
        if let Some(index) = target {
            self.select_box(index);
        }
    }

    /// Rows the wheel moved the view; it leaves the selection where it is.
    fn scroll_by(&mut self, delta: isize) {
        self.scroll = self.scroll.saturating_add_signed(delta);
        self.detached = true;
    }

    /// Fold or unfold the turn the selected box is in, leaving the
    /// selection on its prompt.
    fn toggle_fold(&mut self) {
        let Some(placed) = self.selected_box().map(|index| self.layout.boxes[index]) else {
            return;
        };
        let prompt = self.layout.turn_of(placed.node).unwrap_or(placed.node);
        self.toggle_fold_of(prompt);
    }

    fn toggle_fold_of(&mut self, prompt: usize) {
        let has_steps = self
            .layout
            .box_of(prompt)
            .is_some_and(|index| self.layout.boxes[index].steps > 0);
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
            .boxes
            .iter()
            .filter(|placed| placed.steps > 0)
            .map(|placed| self.nodes[placed.node].id.as_str())
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
            .filter(|&&node| self.layout.box_of(node).is_none())
            .and_then(|&node| self.layout.turn_of(node))
        {
            // The step is folded away: open its turn to land on it.
            self.collapsed.remove(&self.nodes[prompt].id);
        }
        self.selected = Some(target);
        self.came_from = Some(id);
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
        true
    }

    /// The box at a screen cell, and whether the cell is its fold marker.
    fn box_at(&self, column: u16, row: u16) -> Option<(usize, bool)> {
        let view = self.view?;
        let area = view.rows;
        let inside = column >= area.x
            && column < area.x + area.width
            && row >= area.y
            && row < area.y + area.height;
        if !inside {
            return None;
        }
        let flow_row = view.first_row + usize::from(row - area.y);
        let index = flow_row / BOX_ROWS;
        let placed = self.layout.boxes.get(index)?;
        let (x, width) = view.box_columns(placed.column)?;
        let on_title = flow_row % BOX_ROWS == 0;
        (column >= x && column < x + width)
            .then_some((index, on_title && placed.steps > 0 && column == x + 2))
    }
}

/// Place every node: boxes in time order, one column per agent, with
/// agents that worked one after the other sharing a column.
fn build_layout(
    lane_index: &HashMap<String, usize>,
    nodes: &[FlowNode],
    index: &HashMap<String, usize>,
    collapsed: &HashSet<String>,
) -> FlowLayout {
    let lane_count = lane_index.len();
    let mut per_lane: Vec<Vec<usize>> = vec![Vec::new(); lane_count];
    for (at, node) in nodes.iter().enumerate() {
        if let Some(&lane) = lane_index.get(&node.agent_id) {
            per_lane[lane].push(at);
        }
    }

    // Within a lane the record's order holds; across lanes, time does. A
    // lane's clock never runs backwards, so the two cannot disagree.
    let mut order: Vec<(u64, usize, u64, usize)> = Vec::with_capacity(nodes.len());
    let mut turn_of = vec![NO_BOX; nodes.len()];
    let mut steps = vec![0usize; nodes.len()];
    for (lane, list) in per_lane.iter_mut().enumerate() {
        list.sort_by_key(|&at| (nodes[at].seq, at));
        let mut clock = 0;
        let mut turn: Option<usize> = None;
        for &at in list.iter() {
            let node = &nodes[at];
            clock = clock.max(node.at_ms);
            if node.kind == FlowKind::Prompt {
                turn = Some(at);
            } else if let Some(prompt) = turn {
                turn_of[at] = prompt;
                steps[prompt] += 1;
                if collapsed.contains(&nodes[prompt].id) {
                    continue;
                }
            }
            order.push((clock, lane, node.seq, at));
        }
    }
    order.sort_unstable();

    let mut boxes: Vec<FlowBox> = order
        .iter()
        .map(|&(_, lane, _, node)| FlowBox {
            node,
            lane,
            column: 0,
            steps: steps[node],
            folded: steps[node] > 0 && collapsed.contains(&nodes[node].id),
        })
        .collect();
    let mut box_of = vec![NO_BOX; nodes.len()];
    for (placed, flow_box) in boxes.iter().enumerate() {
        box_of[flow_box.node] = placed;
    }
    // A folded step's arrows start and end at its prompt.
    let shown = |node: usize| match box_of[node] {
        NO_BOX => turn_of.get(node).map_or(NO_BOX, |&prompt| {
            box_of.get(prompt).copied().unwrap_or(NO_BOX)
        }),
        placed => placed,
    };

    let mut edges = Vec::new();
    for (at, node) in nodes.iter().enumerate() {
        for (_, other) in &node.links {
            let Some(&other) = index.get(other) else {
                continue;
            };
            let (from, to) = (shown(at), shown(other));
            if from != NO_BOX && to != NO_BOX && boxes[from].lane != boxes[to].lane {
                edges.push(FlowEdge { from, to });
            }
        }
    }
    edges.sort_unstable_by_key(|edge| (edge.to, edge.from));
    edges.dedup();

    let mut span_of_lane = vec![NO_BOX; lane_count];
    let mut spans: Vec<LaneSpan> = Vec::new();
    for (placed, flow_box) in boxes.iter().enumerate() {
        match span_of_lane[flow_box.lane] {
            NO_BOX => {
                span_of_lane[flow_box.lane] = spans.len();
                spans.push(LaneSpan {
                    lane: flow_box.lane,
                    column: 0,
                    first: placed,
                    last: placed,
                });
            }
            span => spans[span].last = placed,
        }
    }
    // A lane's line runs on to wherever its arrows leave it.
    for edge in &edges {
        let span = &mut spans[span_of_lane[boxes[edge.from].lane]];
        span.first = span.first.min(edge.to);
        span.last = span.last.max(edge.to);
    }

    // The agent the flow is of keeps the first column to itself. Every
    // other lane takes the first column whose last lane ended with a box
    // to spare, so the two never touch.
    let mut by_start: Vec<usize> = (0..spans.len()).collect();
    by_start.sort_by_key(|&span| (spans[span].lane != 0, spans[span].first, spans[span].lane));
    let mut ends: Vec<Option<usize>> = vec![None];
    for span in by_start {
        let span = &mut spans[span];
        let column = if span.lane == 0 {
            0
        } else {
            (1..ends.len())
                .find(|&column| ends[column].is_none_or(|end| end + 1 < span.first))
                .unwrap_or(ends.len())
        };
        if column == ends.len() {
            ends.push(None);
        }
        ends[column] = Some(span.last);
        span.column = column;
    }
    for flow_box in &mut boxes {
        flow_box.column = spans[span_of_lane[flow_box.lane]].column;
    }
    spans.sort_by_key(|span| (span.column, span.first));

    FlowLayout {
        boxes,
        spans,
        edges,
        columns: ends.len(),
        box_of,
        turn_of,
    }
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

    /// The wheel over the GRAPH tab.
    pub(super) fn scroll_flow(&mut self, delta: isize) {
        self.flow.scroll_by(delta);
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
        let page = self
            .flow
            .view
            .map_or(1, |view| usize::from(view.rows.height) / BOX_ROWS)
            .saturating_sub(1)
            .max(1);
        let page = isize::try_from(page).unwrap_or(1);
        match key.code {
            KeyCode::Esc => self.show_chat_tab(ChatTab::Chat),
            KeyCode::Up | KeyCode::Char('k') => self.flow.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.flow.move_by(1),
            KeyCode::Left | KeyCode::Char('h') => self.flow.move_across(false),
            KeyCode::Right | KeyCode::Char('l') => self.flow.move_across(true),
            KeyCode::PageUp => self.flow.move_by(-page),
            KeyCode::PageDown => self.flow.move_by(page),
            KeyCode::Home | KeyCode::Char('g') => self.flow.select_box(0),
            KeyCode::End | KeyCode::Char('G') => self.flow.select_box(usize::MAX),
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

    /// A click in the GRAPH tab: on a turn's fold marker it folds the
    /// turn, on a box it selects it, and on the selected box it opens it.
    /// Returns whether the click was on a box.
    pub(super) fn click_flow(&mut self, column: u16, row: u16) -> bool {
        self.flow.ensure_layout();
        let Some((index, on_fold)) = self.flow.box_at(column, row) else {
            return false;
        };
        if on_fold {
            let prompt = self.flow.layout.boxes[index].node;
            self.flow.toggle_fold_of(prompt);
        } else if self.flow.selected_box() == Some(index) {
            self.open_flow_node();
        } else {
            self.flow.select_box(index);
        }
        true
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::file_view::FileBody;
    use crate::events::{AgentSummary, TraceNodeStatus, UiCommand};
    use tokio::sync::mpsc;

    const LEAD: &str = "aa";
    const CODER: &str = "bb";
    const TESTER: &str = "cc";

    fn lane(agent: &str, name: &str, parent: Option<&str>) -> FlowLane {
        FlowLane {
            agent_id: agent.into(),
            name: name.into(),
            parent: parent.map(str::to_string),
        }
    }

    fn node(agent: &str, seq: u64, at_ms: u64, kind: FlowKind, label: &str) -> FlowNode {
        FlowNode {
            id: format!("{agent}:{seq}"),
            agent_id: agent.into(),
            kind,
            label: label.into(),
            status: TraceNodeStatus::Ok,
            seq,
            at_ms,
            ..FlowNode::default()
        }
    }

    fn linked(mut from: FlowNode, kind: &str, to: &str) -> FlowNode {
        from.links.push((kind.into(), to.into()));
        from
    }

    /// A lead that gives a coder a task, gets its reply, then gives a
    /// tester one.
    fn delegation() -> (Vec<FlowLane>, Vec<FlowNode>) {
        let lanes = vec![
            lane(LEAD, "Lead", None),
            lane(CODER, "coder #1", Some(LEAD)),
            lane(TESTER, "tester #1", Some(LEAD)),
        ];
        let nodes = vec![
            node(LEAD, 1, 100, FlowKind::Prompt, "add a greeting"),
            node(LEAD, 2, 110, FlowKind::Model, "sonnet"),
            linked(
                node(LEAD, 3, 120, FlowKind::Tool, "task coder"),
                "spawn",
                "bb:1",
            ),
            node(CODER, 1, 130, FlowKind::Prompt, "write greet()"),
            node(CODER, 2, 140, FlowKind::Tool, "write_file src/greet.rs"),
            linked(
                node(CODER, 3, 150, FlowKind::Reply, "done"),
                "return",
                "aa:6",
            ),
            node(LEAD, 6, 160, FlowKind::Result, "coder #1 replied"),
            linked(
                node(LEAD, 7, 170, FlowKind::Tool, "task tester"),
                "spawn",
                "cc:1",
            ),
            node(TESTER, 1, 180, FlowKind::Prompt, "test greet()"),
            linked(
                node(TESTER, 2, 190, FlowKind::Reply, "passes"),
                "return",
                "aa:9",
            ),
            node(LEAD, 9, 200, FlowKind::Result, "tester #1 replied"),
            node(LEAD, 10, 210, FlowKind::Reply, "greeting added"),
        ];
        (lanes, nodes)
    }

    fn app_with_flow() -> App {
        let mut app = App::new();
        app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
        app.show_chat_tab(ChatTab::Graph);
        let (lanes, nodes) = delegation();
        app.process_command(UiCommand::FlowLoad {
            agent_id: LEAD.into(),
            lanes,
            nodes,
        });
        app.flow_mut().ensure_layout();
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::from(code));
        app.flow_mut().ensure_layout();
    }

    fn selected(app: &App) -> &str {
        app.flow()
            .selected_node()
            .map_or("", |node| node.id.as_str())
    }

    fn labels(app: &App) -> Vec<&str> {
        let flow = app.flow();
        flow.layout()
            .boxes
            .iter()
            .map(|placed| flow.nodes()[placed.node].label.as_str())
            .collect()
    }

    #[test]
    fn boxes_run_in_time_order_and_agents_that_take_turns_share_a_column() {
        let app = app_with_flow();
        let layout = app.flow().layout();
        let columns: Vec<usize> = layout.boxes.iter().map(|placed| placed.column).collect();
        assert_eq!(columns, [0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 0, 0]);
        assert_eq!(layout.columns, 2, "the tester reuses the coder's column");
        assert_eq!(
            layout.edges,
            [
                FlowEdge { from: 2, to: 3 },
                FlowEdge { from: 5, to: 6 },
                FlowEdge { from: 7, to: 8 },
                FlowEdge { from: 9, to: 10 },
            ]
        );
        assert_eq!(layout.rows(), 25);
        // The coder's line runs on to the lead's box its reply went to.
        let coder = layout.spans.iter().find(|span| span.lane == 1).unwrap();
        assert_eq!((coder.first, coder.last), (3, 6));
    }

    #[test]
    fn agents_working_at_once_get_a_column_each() {
        let mut app = App::new();
        app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
        app.show_chat_tab(ChatTab::Graph);
        app.process_command(UiCommand::FlowLoad {
            agent_id: LEAD.into(),
            lanes: vec![
                lane(LEAD, "Lead", None),
                lane(CODER, "coder #1", Some(LEAD)),
                lane(TESTER, "tester #1", Some(LEAD)),
            ],
            nodes: vec![
                node(LEAD, 1, 100, FlowKind::Prompt, "do both"),
                node(CODER, 1, 110, FlowKind::Prompt, "code"),
                node(TESTER, 1, 120, FlowKind::Prompt, "test"),
                node(CODER, 2, 130, FlowKind::Reply, "coded"),
                node(TESTER, 2, 140, FlowKind::Reply, "tested"),
            ],
        });
        app.flow_mut().ensure_layout();
        let columns: Vec<usize> = app
            .flow()
            .layout()
            .boxes
            .iter()
            .map(|placed| placed.column)
            .collect();
        assert_eq!(columns, [0, 1, 2, 1, 2]);
    }

    #[test]
    fn a_lane_keeps_its_record_order_when_clocks_disagree() {
        let mut app = App::new();
        app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
        app.show_chat_tab(ChatTab::Graph);
        app.process_command(UiCommand::FlowLoad {
            agent_id: LEAD.into(),
            lanes: vec![lane(LEAD, "Lead", None)],
            nodes: vec![
                node(LEAD, 2, 90, FlowKind::Model, "second"),
                node(LEAD, 1, 100, FlowKind::Prompt, "first"),
                node(LEAD, 3, 95, FlowKind::Reply, "third"),
            ],
        });
        app.flow_mut().ensure_layout();
        assert_eq!(labels(&app), ["first", "second", "third"]);
    }

    #[test]
    fn opening_the_tab_asks_for_the_flow_of_the_agent_on_screen() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut app = App::new().with_event_sender(tx);
        app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
        assert!(rx.try_recv().is_err(), "nothing asked while on CHAT");

        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(app.chat_tab(), ChatTab::Graph);
        assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenFlow(id)) if id == LEAD));
        assert!(!app.flow().loaded());

        // Another agent's flow is not this tab's.
        let (lanes, nodes) = delegation();
        app.process_command(UiCommand::FlowLoad {
            agent_id: CODER.into(),
            lanes: lanes.clone(),
            nodes: nodes.clone(),
        });
        assert!(!app.flow().loaded());
        app.process_command(UiCommand::FlowLoad {
            agent_id: LEAD.into(),
            lanes,
            nodes,
        });
        assert!(app.flow().loaded());

        // Switching agents shows the new agent's flow.
        app.process_command(UiCommand::SetActiveAgent(CODER.into()));
        assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenFlow(id)) if id == CODER));
        assert!(app.flow().nodes().is_empty());

        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.chat_tab(), ChatTab::Chat);
    }

    #[test]
    fn the_selection_follows_new_boxes_until_it_is_moved() {
        let mut app = app_with_flow();
        assert_eq!(selected(&app), "aa:10", "starts on the newest box");

        let more = |seq: u64| UiCommand::FlowUpsert {
            agent_id: LEAD.into(),
            lanes: Vec::new(),
            nodes: vec![node(
                LEAD,
                seq,
                300 + seq,
                FlowKind::Prompt,
                "and a farewell",
            )],
        };
        app.process_command(more(11));
        app.flow_mut().ensure_layout();
        assert_eq!(selected(&app), "aa:11");

        press(&mut app, KeyCode::Up);
        assert_eq!(selected(&app), "aa:10");
        app.process_command(more(12));
        app.flow_mut().ensure_layout();
        assert_eq!(selected(&app), "aa:10", "stays where it was put");

        press(&mut app, KeyCode::End);
        app.process_command(more(13));
        app.flow_mut().ensure_layout();
        assert_eq!(selected(&app), "aa:13");
    }

    #[test]
    fn an_update_changes_the_node_it_is_about() {
        let mut app = app_with_flow();
        let mut update = node(CODER, 2, 0, FlowKind::Tool, "");
        update.status = TraceNodeStatus::Error;
        update.detail = "denied".into();
        app.process_command(UiCommand::FlowUpsert {
            agent_id: LEAD.into(),
            lanes: Vec::new(),
            nodes: vec![update],
        });
        let flow = app.flow();
        assert_eq!(flow.nodes().len(), 12);
        let tool = flow.nodes().iter().find(|n| n.id == "bb:2").unwrap();
        assert_eq!(tool.label, "write_file src/greet.rs");
        assert_eq!(tool.detail, "denied");
        assert_eq!(tool.status, TraceNodeStatus::Error);

        // Updates for another agent's flow are not this tab's.
        app.process_command(UiCommand::FlowUpsert {
            agent_id: CODER.into(),
            lanes: Vec::new(),
            nodes: vec![node(CODER, 9, 900, FlowKind::Reply, "stray")],
        });
        assert_eq!(app.flow().nodes().len(), 12);
    }

    #[test]
    fn arrows_move_in_time_and_across_lanes() {
        let mut app = app_with_flow();
        press(&mut app, KeyCode::Home);
        assert_eq!(selected(&app), "aa:1");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(selected(&app), "aa:3");
        press(&mut app, KeyCode::Right);
        assert_eq!(selected(&app), "bb:1", "the nearest box of the next lane");
        press(&mut app, KeyCode::Char(']'));
        assert_eq!(selected(&app), "cc:1", "the next prompt");
        press(&mut app, KeyCode::Char('['));
        assert_eq!(selected(&app), "bb:1");
        press(&mut app, KeyCode::Left);
        assert_eq!(selected(&app), "aa:3");
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(selected(&app), "aa:10");
    }

    #[test]
    fn following_links_walks_the_delegation_and_back() {
        let mut app = app_with_flow();
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Char('f'));
        assert_eq!(selected(&app), "aa:1");
        assert!(app.notification().is_some(), "a prompt links nowhere");

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('f'));
        assert_eq!(
            selected(&app),
            "bb:1",
            "the task call to the agent it started"
        );
        press(&mut app, KeyCode::Char('f'));
        assert_eq!(selected(&app), "aa:3", "and back");
    }

    #[test]
    fn folding_a_turn_leaves_its_prompt_and_moves_its_arrows_there() {
        let mut app = app_with_flow();
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Down);
        assert_eq!(selected(&app), "bb:2");

        press(&mut app, KeyCode::Char(' '));
        assert_eq!(selected(&app), "bb:1", "the selection moves to the prompt");
        let layout = app.flow().layout();
        let prompt = layout.boxes[3];
        assert!(prompt.folded);
        assert_eq!(prompt.steps, 2);
        assert_eq!(layout.boxes.len(), 10);
        assert!(
            layout.edges.contains(&FlowEdge { from: 3, to: 4 }),
            "the reply's arrow leaves from the prompt: {:?}",
            layout.edges
        );

        // A link into the folded turn opens it again.
        press(&mut app, KeyCode::Down);
        assert_eq!(selected(&app), "aa:6");
        press(&mut app, KeyCode::Char('f'));
        assert_eq!(selected(&app), "bb:3");
        assert_eq!(app.flow().layout().boxes.len(), 12);

        press(&mut app, KeyCode::Char('z'));
        assert_eq!(
            labels(&app),
            ["add a greeting", "write greet()", "test greet()"],
            "every turn folded"
        );
        assert_eq!(selected(&app), "bb:1");
        press(&mut app, KeyCode::Char('z'));
        assert_eq!(app.flow().layout().boxes.len(), 12);
    }

    #[test]
    fn enter_opens_the_node_as_json_and_closing_it_comes_back() {
        let mut app = app_with_flow();
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        app.flow_mut().nodes[2].meta = vec![("args".into(), r#"{"agent":"coder"}"#.into())];
        press(&mut app, KeyCode::Enter);
        let Some(DetailView::File(file)) = app.detail_view() else {
            panic!("not a file view: {:?}", app.detail_view());
        };
        assert_eq!(file.title, "GRAPH TOOL task coder");
        let FileBody::Text(json) = &file.body else {
            panic!("no text");
        };
        assert!(json.contains("\"agent\": \"Lead\""), "{json}");
        assert!(
            json.contains("\"args\": {\n      \"agent\": \"coder\""),
            "{json}"
        );
        assert!(json.contains("\"kind\": \"spawn\""), "{json}");
        assert!(json.contains("\"label\": \"write greet()\""), "{json}");

        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert!(app.detail_view().is_none());
        assert_eq!(app.chat_tab(), ChatTab::Graph);
        assert_eq!(selected(&app), "aa:3");
    }

    #[test]
    fn o_opens_the_agent_of_the_selected_box() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut app = App::new().with_event_sender(tx);
        let listed = |id: &str, name: &str| AgentSummary {
            id: id.into(),
            name: name.into(),
            ..AgentSummary::default()
        };
        app.process_command(UiCommand::SetAgents(vec![
            listed(LEAD, "Lead"),
            listed(CODER, "coder #1"),
        ]));
        app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
        app.show_chat_tab(ChatTab::Graph);
        let (lanes, nodes) = delegation();
        app.process_command(UiCommand::FlowLoad {
            agent_id: LEAD.into(),
            lanes,
            nodes,
        });
        while rx.try_recv().is_ok() {}

        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('o'));
        assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == CODER));

        // The tester runs in the background and has no chat.
        press(&mut app, KeyCode::Char(']'));
        assert_eq!(selected(&app), "cc:1");
        press(&mut app, KeyCode::Char('o'));
        assert!(rx.try_recv().is_err());
        assert!(app.notification().is_some());

        press(&mut app, KeyCode::Char('r'));
        let (message, _) = app.notification().unwrap();
        assert!(message.contains("tester #1"), "{message}");
    }

    #[test]
    fn typing_still_reaches_the_input() {
        let mut app = app_with_flow();
        press(&mut app, KeyCode::Char('/'));
        assert_eq!(app.input(), "/");
        // Once the line has text, the flow's letters are text too.
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.input(), "/g");
        assert_eq!(selected(&app), "aa:10");
    }

    #[test]
    fn old_nodes_go_once_the_flow_is_too_long() {
        let mut app = App::new();
        app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
        app.show_chat_tab(ChatTab::Graph);
        let nodes = (1..=u64::try_from(MAX_NODES).unwrap() + 1)
            .map(|seq| node(LEAD, seq, seq, FlowKind::Model, "call"))
            .collect();
        app.process_command(UiCommand::FlowLoad {
            agent_id: LEAD.into(),
            lanes: vec![lane(LEAD, "Lead", None)],
            nodes,
        });
        let flow = app.flow_mut();
        flow.ensure_layout();
        assert_eq!(flow.nodes().len(), MAX_NODES * 3 / 4);
        assert_eq!(flow.layout().boxes.len(), flow.nodes().len());
        assert_eq!(
            flow.selected_node().map(|n| n.seq),
            u64::try_from(MAX_NODES + 1).ok()
        );
    }
}
