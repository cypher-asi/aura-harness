//! Run graph over the chat panel: the run → turn → iteration → step
//! hierarchy of the active or a recorded run, the links between steps
//! (which decision shaped which model call, which result answered which
//! call, which steps touched the same file), and each node's metadata.

use std::collections::{HashMap, HashSet};

use super::{follow_selection, App, DetailView};
use crate::events::{TraceNode, TraceNodeStatus, UiEvent};
use crossterm::event::{KeyCode, KeyEvent};

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

impl App {
    /// The run graph.
    #[must_use]
    pub const fn graph(&self) -> &GraphState {
        &self.graph
    }

    /// The run graph, for the renderer to scroll.
    pub fn graph_mut(&mut self) -> &mut GraphState {
        &mut self.graph
    }

    pub(super) fn load_graph(&mut self, run_id: String, nodes: Vec<TraceNode>) {
        self.graph.reset(run_id);
        for node in nodes {
            self.graph.upsert(node);
        }
        let rows = self.graph.rows();
        let last = rows.len().saturating_sub(1);
        self.graph.select_row(&rows, last);
        self.open_detail(DetailView::Graph);
    }

    pub(super) fn upsert_graph(&mut self, run_id: &str, nodes: Vec<TraceNode>) {
        if self.graph.run_id != run_id {
            return;
        }
        for node in nodes {
            self.graph.upsert(node);
        }
    }

    pub(super) fn request_graph(&self, arg: &str) {
        if let Some(tx) = &self.event_tx {
            let id = (!arg.is_empty()).then(|| arg.to_string());
            let _ = tx.try_send(UiEvent::OpenGraph(id));
        }
    }

    /// Keys of the graph view. Returns `true` when the view should close.
    pub(super) fn handle_graph_key(&mut self, key: KeyEvent) -> bool {
        let page = self.detail_page_rows().max(1);
        let g = &mut self.graph;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return true,
            KeyCode::Up | KeyCode::Char('k') => g.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => g.move_by(1),
            KeyCode::PageUp => g.move_by(-isize::try_from(page).unwrap_or(10)),
            KeyCode::PageDown => g.move_by(isize::try_from(page).unwrap_or(10)),
            KeyCode::Home | KeyCode::Char('g') => {
                let rows = g.rows();
                g.select_row(&rows, 0);
            }
            KeyCode::End | KeyCode::Char('G') => {
                let rows = g.rows();
                g.select_row(&rows, usize::MAX);
            }
            KeyCode::Left | KeyCode::Char('h') => g.collapse_or_parent(),
            KeyCode::Right | KeyCode::Char('l') => g.expand_or_child(),
            KeyCode::Enter | KeyCode::Char(' ') => g.toggle(),
            KeyCode::Tab => g.follow_link(),
            KeyCode::Char('e') => g.issues_only = !g.issues_only,
            _ => {}
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::UiCommand;

    fn node(id: &str, parent: Option<&str>, status: TraceNodeStatus) -> TraceNode {
        TraceNode {
            id: id.into(),
            kind: "step".into(),
            parent: parent.map(str::to_string),
            label: id.into(),
            status,
            ..TraceNode::default()
        }
    }

    fn loaded() -> App {
        let mut app = App::new();
        let mut decision = node("s2", Some("t1"), TraceNodeStatus::Warn);
        decision.links = vec![("triggered".into(), "s3".into())];
        app.process_command(UiCommand::GraphLoad {
            run_id: "r".into(),
            nodes: vec![
                node("run", None, TraceNodeStatus::Ok),
                node("t1", Some("run"), TraceNodeStatus::Ok),
                decision,
                node("s3", Some("t1"), TraceNodeStatus::Ok),
            ],
        });
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    #[test]
    fn load_opens_view_and_selects_latest() {
        let app = loaded();
        assert_eq!(app.detail_view(), Some(&DetailView::Graph));
        assert_eq!(app.graph().selected_node().unwrap().id, "s3");
        let rows = app.graph().rows();
        assert_eq!(rows.len(), 4);
        assert!(rows[3].last);
        assert_eq!(rows[2].guides, vec![false, false]);
    }

    #[test]
    fn links_are_bidirectional_and_tab_follows_them() {
        let mut app = loaded();
        assert!(app.graph().linked_to_selected().contains("s2"));
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.graph().selected_node().unwrap().id, "s2");
    }

    #[test]
    fn collapse_expand_and_issue_filter() {
        let mut app = loaded();
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.graph().selected_node().unwrap().id, "t1");
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.graph().rows().len(), 2);
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.graph().rows().len(), 4);
        app.handle_key(key(KeyCode::Char('e')));
        let ids: Vec<&str> = app
            .graph()
            .rows()
            .iter()
            .map(|r| app.graph().nodes()[r.node].id.as_str())
            .collect();
        assert_eq!(ids, vec!["run", "t1", "s2"]);
        app.handle_key(key(KeyCode::Esc));
        assert!(app.detail_view().is_none());
    }

    #[test]
    fn upserts_for_other_runs_are_ignored() {
        let mut app = loaded();
        app.process_command(UiCommand::GraphUpsert {
            run_id: "other".into(),
            nodes: vec![node("s9", Some("t1"), TraceNodeStatus::Ok)],
        });
        assert!(app.graph().node("s9").is_none());
        let mut updated = node("s3", Some("t1"), TraceNodeStatus::Error);
        updated.detail = "failed".into();
        app.process_command(UiCommand::GraphUpsert {
            run_id: "r".into(),
            nodes: vec![updated, node("s4", Some("t1"), TraceNodeStatus::Pending)],
        });
        assert_eq!(
            app.graph().node("s3").unwrap().status,
            TraceNodeStatus::Error
        );
        assert_eq!(app.graph().rows().len(), 5);
    }
}
