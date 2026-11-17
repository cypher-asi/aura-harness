//! Run graph over the chat panel: the run → turn → iteration → step
//! hierarchy of the active or a recorded run, the links between steps
//! (which decision shaped which model call, which result answered which
//! call, which steps touched the same file), and each node's metadata.

use std::collections::{HashMap, HashSet};

use super::file_view::FileView;
use super::{follow_selection, App, DetailView, NotificationType, PanelFocus};
use crate::events::{TraceNode, TraceNodeStatus, UiEvent};
use crossterm::event::{KeyCode, KeyEvent};
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
        if let Some(id) = self.pending_graph_node.take() {
            self.graph.select_id(&id);
        }
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
        let page = isize::try_from(page).unwrap_or(10);
        let g = &mut self.graph;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return true,
            KeyCode::Up | KeyCode::Char('k') => g.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => g.move_by(1),
            KeyCode::PageUp => g.move_by(-page),
            KeyCode::PageDown => g.move_by(page),
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
            KeyCode::Enter => self.inspect_selected_node(),
            KeyCode::Char(' ') => g.toggle(),
            KeyCode::Tab => g.follow_link(),
            KeyCode::Char('e') => g.issues_only = !g.issues_only,
            KeyCode::Char('+' | '=') => self.label_selected(1.0),
            KeyCode::Char('-') => self.label_selected(-1.0),
            KeyCode::Char('r') => self.jump_to_record(),
            KeyCode::Char('o') => self.open_node_file(),
            _ => {}
        }
        false
    }

    /// A click on a tree row selects it and opens it full panel.
    pub(crate) fn click_graph(&mut self, column: u16, row: u16) {
        let g = &mut self.graph;
        let Some(index) = g.row_at(column, row) else {
            return;
        };
        let rows = g.rows();
        if index < rows.len() {
            g.select_row(&rows, index);
            self.inspect_selected_node();
        }
    }

    /// Show the selected node as highlighted JSON over the chat panel, the
    /// way a file opens. Closing it comes back to the graph.
    pub(super) fn inspect_selected_node(&mut self) {
        let Some(node) = self.graph.selected_node() else {
            return;
        };
        let links: Vec<super::trace_json::NodeLink> = self
            .graph
            .links_of(&node.id)
            .iter()
            .map(|(kind, other)| {
                let label = self.graph.node(other).map(|n| n.label.clone());
                (kind.clone(), other.clone(), label)
            })
            .collect();
        let json = super::trace_json::node_json(node, &links);
        let title = node_title(node);
        let view = FileView::snapshot(&title, &json, "json", 1);
        self.open_detail(DetailView::File(Box::new(view)));
        self.graph.behind = true;
    }

    /// After a file view closes: back to the graph when it was opened
    /// from there. Returns whether the graph came back.
    pub(super) fn return_to_graph(&mut self) -> bool {
        if std::mem::take(&mut self.graph.behind) {
            self.open_detail(DetailView::Graph);
            return true;
        }
        false
    }

    /// Label the selected step (or the run, on the run node) good or bad.
    fn label_selected(&mut self, value: f64) {
        let Some(node) = self.graph.selected_node() else {
            return;
        };
        let id = node.id.clone();
        let seq = if node.kind == "run" { None } else { node.seq };
        if node.kind != "run" && seq.is_none() {
            self.notification = Some((
                "This node has no step to label".into(),
                NotificationType::Warning,
            ));
            return;
        }
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::LabelStep {
                run_id: self.graph.run_id.clone(),
                seq,
                value,
            });
        }
        self.graph.set_meta(&id, "human", format!("{value:+}"));
    }

    /// Show the selected node's first kernel record row in the Record
    /// panel.
    fn jump_to_record(&mut self) {
        let seqs = self
            .graph
            .selected_node()
            .and_then(|n| meta(n, "record_seqs"))
            .map(|v| numbers(&v))
            .unwrap_or_default();
        let found = seqs
            .iter()
            .find_map(|seq| self.records.iter().position(|r| r.seq == *seq));
        match (seqs.first(), found) {
            (None, _) => {
                self.notification = Some((
                    "No kernel record linked to this node".into(),
                    NotificationType::Warning,
                ));
            }
            (Some(seq), None) => {
                self.notification = Some((
                    format!("Record #{seq} is not in the Record panel (another agent or session)"),
                    NotificationType::Warning,
                ));
            }
            (Some(_), Some(index)) => {
                self.selected_record = index;
                self.focus = PanelFocus::Records;
                self.open_detail(DetailView::Record);
            }
        }
    }

    /// Open the first file the selected node touched.
    fn open_node_file(&mut self) {
        let path = self.graph.selected_node().and_then(|n| {
            meta(n, "path").filter(|p| !p.is_empty()).or_else(|| {
                meta(n, "files").and_then(|v| {
                    serde_json::from_str::<Vec<String>>(&v)
                        .ok()
                        .and_then(|f| f.into_iter().next())
                })
            })
        });
        let Some(path) = path else {
            self.notification = Some((
                "No file linked to this node".into(),
                NotificationType::Warning,
            ));
            return;
        };
        let root = self.sandbox.root().map(std::path::Path::to_path_buf);
        let full = match &root {
            Some(root) if std::path::Path::new(&path).is_relative() => root.join(&path),
            _ => std::path::PathBuf::from(&path),
        };
        let view = FileView::load(&full, root.as_deref());
        self.open_detail(DetailView::File(Box::new(view)));
        self.graph.behind = true;
    }
}

fn meta(node: &TraceNode, key: &str) -> Option<String> {
    node.meta
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

fn numbers(text: &str) -> Vec<u64> {
    text.split(|c: char| !c.is_ascii_digit())
        .filter_map(|s| s.parse().ok())
        .collect()
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
    fn labels_are_sent_and_shown() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let mut app = App::new().with_event_sender(tx);
        let mut step = node("s3", Some("run"), TraceNodeStatus::Ok);
        step.seq = Some(3);
        app.process_command(UiCommand::GraphLoad {
            run_id: "r".into(),
            nodes: vec![node("run", None, TraceNodeStatus::Ok), step],
        });
        app.handle_key(key(KeyCode::Char('-')));
        match rx.try_recv().unwrap() {
            UiEvent::LabelStep { run_id, seq, value } => {
                assert_eq!((run_id.as_str(), seq, value), ("r", Some(3), -1.0));
            }
            other => panic!("unexpected {other:?}"),
        }
        let shown = meta(app.graph().node("s3").unwrap(), "human");
        assert_eq!(shown.as_deref(), Some("-1"));
    }

    #[test]
    fn r_jumps_to_the_linked_record_row() {
        let mut app = App::new();
        app.records.push_back(crate::app::detail_tests::record(4));
        app.records.push_back(crate::app::detail_tests::record(7));
        let mut step = node("tc:a", Some("run"), TraceNodeStatus::Ok);
        step.meta = vec![("record_seqs".into(), "[7,8]".into())];
        app.process_command(UiCommand::GraphLoad {
            run_id: "r".into(),
            nodes: vec![node("run", None, TraceNodeStatus::Ok), step],
        });
        app.handle_key(key(KeyCode::Char('r')));
        assert_eq!(app.detail_view(), Some(&DetailView::Record));
        assert_eq!(app.selected_record(), 1);
    }

    #[test]
    fn feed_entries_open_as_json_and_g_opens_the_graph() {
        let mut app = loaded();
        app.process_command(UiCommand::GraphUpsert {
            run_id: "r".into(),
            nodes: vec![TraceNode {
                kind: "decision".into(),
                ..node("s2", Some("t1"), TraceNodeStatus::Warn)
            }],
        });
        assert_eq!(app.trace_feed().entries().len(), 1);
        app.close_detail();
        app.open_selected_trace_entry();
        let Some(DetailView::File(view)) = app.detail_view() else {
            panic!("the step opens as a file view");
        };
        assert!(view.title.starts_with("TRACE DECISION"), "{}", view.title);
        assert!(
            matches!(&view.body, crate::app::file_view::FileBody::Text(t) if t.contains("\"s2\""))
        );

        app.close_detail();
        app.open_selected_trace_in_graph();
        assert_eq!(app.detail_view(), Some(&DetailView::Graph));
        assert_eq!(app.graph().selected_node().unwrap().id, "s2");
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
