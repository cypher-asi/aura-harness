//! Run graph over the chat panel: the run → turn → iteration → step
//! hierarchy of the active or a recorded run, the links between steps
//! (which decision shaped which model call, which result answered which
//! call, which steps touched the same file), and each node's metadata.

mod actions;

use std::collections::{HashMap, HashSet};

use super::follow_selection;
use crate::events::{TraceNode, TraceNodeStatus};
use ratatui::layout::Rect;

/// Title of a step opened as JSON, from the graph or the TRACE tab.
pub(super) fn node_title(node: &TraceNode) -> String {
    format!("TRACE {} {}", node.kind.to_uppercase(), node.label)
}

/// One visible row of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    /// Index into [`GraphState::nodes`]
    pub node: usize,
    /// For each ancestor level, whether a sibling follows below (draws `│`)
    pub guides: Vec<bool>,
    /// Whether this is the last child of its parent
    pub last: bool,
    /// Whether the node has children
    pub has_children: bool,
    /// Whether its children are hidden
    pub collapsed: bool,
}

/// Run graph shown by `/graph`.
#[derive(Debug, Default)]
pub struct GraphState {
    run_id: String,
    nodes: Vec<TraceNode>,
    index: HashMap<String, usize>,
    children: HashMap<String, Vec<String>>,
    roots: Vec<String>,
    links: HashMap<String, Vec<(String, String)>>,
    collapsed: HashSet<String>,
    issues_only: bool,
    selected: Option<String>,
    scroll: usize,
    link_cursor: usize,
    /// Whether the open file view came from the graph, so closing it
    /// comes back here
    pub(super) behind: bool,
    /// Tree rows drawn in the last frame, for mouse hits
    tree_area: Option<Rect>,
}

impl GraphState {
    /// Run shown.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// All nodes, in arrival order.
    #[must_use]
    pub fn nodes(&self) -> &[TraceNode] {
        &self.nodes
    }

    /// Node by id.
    #[must_use]
    pub fn node(&self, id: &str) -> Option<&TraceNode> {
        self.index.get(id).map(|&i| &self.nodes[i])
    }

    /// Whether only warning and error paths are shown.
    #[must_use]
    pub const fn issues_only(&self) -> bool {
        self.issues_only
    }

    /// Links of `id` in both directions: (edge kind, other node). Edges
    /// pointing at `id` are prefixed with `←`.
    #[must_use]
    pub fn links_of(&self, id: &str) -> &[(String, String)] {
        self.links.get(id).map_or(&[], Vec::as_slice)
    }

    /// Ids linked to the selected node.
    #[must_use]
    pub fn linked_to_selected(&self) -> HashSet<&str> {
        self.selected
            .as_deref()
            .map(|id| {
                self.links_of(id)
                    .iter()
                    .map(|(_, other)| other.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Selected node.
    #[must_use]
    pub fn selected_node(&self) -> Option<&TraceNode> {
        self.selected.as_deref().and_then(|id| self.node(id))
    }

    /// Remember where the tree rows were drawn.
    pub fn set_tree_area(&mut self, tree: Option<Rect>) {
        self.tree_area = tree;
    }

    /// Row index of the tree at a screen cell.
    fn row_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.tree_area?;
        let inside = column >= area.x
            && column < area.x + area.width
            && row >= area.y
            && row < area.y + area.height;
        inside.then(|| self.scroll + usize::from(row - area.y))
    }

    fn reset(&mut self, run_id: String) {
        *self = Self {
            run_id,
            ..Self::default()
        };
    }

    fn upsert(&mut self, node: TraceNode) {
        for (kind, other) in &node.links {
            let forward = self.links.entry(node.id.clone()).or_default();
            if !forward.iter().any(|(k, o)| k == kind && o == other) {
                forward.push((kind.clone(), other.clone()));
            }
            let reverse_kind = format!("←{kind}");
            let back = self.links.entry(other.clone()).or_default();
            if !back
                .iter()
                .any(|(k, o)| *k == reverse_kind && *o == node.id)
            {
                back.push((reverse_kind, node.id.clone()));
            }
        }
        if let Some(&i) = self.index.get(&node.id) {
            self.nodes[i] = node;
            return;
        }
        match &node.parent {
            Some(parent) => self
                .children
                .entry(parent.clone())
                .or_default()
                .push(node.id.clone()),
            None => self.roots.push(node.id.clone()),
        }
        self.index.insert(node.id.clone(), self.nodes.len());
        self.nodes.push(node);
    }

    fn interesting(&self) -> HashSet<String> {
        fn visit(g: &GraphState, id: &str, keep: &mut HashSet<String>) -> bool {
            let own = g.node(id).is_some_and(|n| {
                matches!(n.status, TraceNodeStatus::Warn | TraceNodeStatus::Error)
            });
            let mut any = own;
            for child in g.children.get(id).into_iter().flatten() {
                any |= visit(g, child, keep);
            }
            if any {
                keep.insert(id.to_string());
            }
            any
        }
        let mut keep = HashSet::new();
        for root in &self.roots {
            visit(self, root, &mut keep);
        }
        keep
    }

    /// Visible rows in display order.
    #[must_use]
    pub fn rows(&self) -> Vec<GraphRow> {
        let keep = self.issues_only.then(|| self.interesting());
        let mut rows = Vec::new();
        let mut stack: Vec<(String, Vec<bool>, bool)> = Vec::new();
        let roots: Vec<&String> = self
            .roots
            .iter()
            .filter(|id| keep.as_ref().is_none_or(|k| k.contains(*id)))
            .collect();
        for (i, root) in roots.iter().enumerate().rev() {
            stack.push(((*root).clone(), Vec::new(), i + 1 == roots.len()));
        }
        while let Some((id, guides, last)) = stack.pop() {
            let Some(&node) = self.index.get(&id) else {
                continue;
            };
            let kids: Vec<&String> = self
                .children
                .get(&id)
                .into_iter()
                .flatten()
                .filter(|c| keep.as_ref().is_none_or(|k| k.contains(*c)))
                .collect();
            let collapsed = self.collapsed.contains(&id);
            if !collapsed {
                let mut child_guides = guides.clone();
                child_guides.push(!last);
                for (i, child) in kids.iter().enumerate().rev() {
                    stack.push(((*child).clone(), child_guides.clone(), i + 1 == kids.len()));
                }
            }
            rows.push(GraphRow {
                node,
                guides,
                last,
                has_children: !kids.is_empty(),
                collapsed,
            });
        }
        rows
    }

    /// Position of the selected node among `rows`, or 0.
    #[must_use]
    pub fn selected_row(&self, rows: &[GraphRow]) -> usize {
        self.selected
            .as_deref()
            .and_then(|id| rows.iter().position(|r| self.nodes[r.node].id == id))
            .unwrap_or(0)
    }

    /// Keep the selection inside `visible` rows, returning the first row
    /// to draw.
    pub fn sync_scroll(&mut self, rows: &[GraphRow], visible: usize) -> usize {
        let selected = self.selected_row(rows);
        self.scroll = follow_selection(self.scroll, selected, visible, rows.len());
        self.scroll
    }

    fn select_row(&mut self, rows: &[GraphRow], row: usize) {
        if let Some(r) = rows.get(row.min(rows.len().saturating_sub(1))) {
            self.selected = Some(self.nodes[r.node].id.clone());
            self.link_cursor = 0;
        }
    }

    fn move_by(&mut self, delta: isize) {
        let rows = self.rows();
        let at = self.selected_row(&rows);
        self.select_row(&rows, at.saturating_add_signed(delta));
    }

    /// Select `id`, expanding its ancestors.
    pub(super) fn select_id(&mut self, id: &str) {
        if self.node(id).is_some() {
            self.reveal(id);
            self.link_cursor = 0;
        }
    }

    fn set_meta(&mut self, id: &str, key: &str, value: String) {
        let Some(&i) = self.index.get(id) else {
            return;
        };
        let meta = &mut self.nodes[i].meta;
        match meta.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => meta.push((key.to_string(), value)),
        }
    }

    /// Show `id`, expanding its ancestors and selecting it.
    fn reveal(&mut self, id: &str) {
        let mut cursor = self.node(id).and_then(|n| n.parent.clone());
        while let Some(parent) = cursor {
            self.collapsed.remove(&parent);
            cursor = self.node(&parent).and_then(|n| n.parent.clone());
        }
        if self.issues_only && !self.interesting().contains(id) {
            self.issues_only = false;
        }
        self.selected = Some(id.to_string());
    }

    fn collapse_or_parent(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let has_children = self.children.get(&id).is_some_and(|c| !c.is_empty());
        if has_children && !self.collapsed.contains(&id) {
            self.collapsed.insert(id);
        } else if let Some(parent) = self.node(&id).and_then(|n| n.parent.clone()) {
            self.selected = Some(parent);
        }
    }

    fn expand_or_child(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if self.collapsed.remove(&id) {
            return;
        }
        if let Some(first) = self.children.get(&id).and_then(|c| c.first()).cloned() {
            self.selected = Some(first);
        }
    }

    fn toggle(&mut self) {
        if let Some(id) = self.selected.clone() {
            if !self.collapsed.remove(&id) {
                self.collapsed.insert(id);
            }
        }
    }

    fn follow_link(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let links = self.links_of(&id).to_vec();
        if links.is_empty() {
            return;
        }
        let (_, target) = &links[self.link_cursor % links.len()];
        let next_cursor = self.link_cursor + 1;
        self.reveal(&target.clone());
        self.link_cursor = next_cursor;
    }
}

#[cfg(test)]
mod tests;
