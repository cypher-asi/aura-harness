//! The graph view's commands: loading it, its keys and clicks, and
//! what they open.

use super::{node_title, GraphState};
use crate::app::file_view::FileView;
use crate::app::{App, DetailView, NotificationType, PanelFocus};
use crate::events::{TraceNode, UiEvent};
use crossterm::event::{KeyCode, KeyEvent};

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

    pub(in crate::app) fn load_graph(&mut self, run_id: String, nodes: Vec<TraceNode>) {
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

    /// Add `nodes` to the run graph when it holds run `run_id`. Returns
    /// whether it does.
    pub(in crate::app) fn upsert_graph(&mut self, run_id: &str, nodes: Vec<TraceNode>) -> bool {
        if self.graph.run_id != run_id {
            return false;
        }
        for node in nodes {
            self.graph.upsert(node);
        }
        true
    }

    pub(in crate::app) fn request_graph(&self, arg: &str) {
        if let Some(tx) = &self.event_tx {
            let id = (!arg.is_empty()).then(|| arg.to_string());
            let _ = tx.try_send(UiEvent::OpenGraph(id));
        }
    }

    /// Keys of the graph view. Returns `true` when the view should close.
    pub(in crate::app) fn handle_graph_key(&mut self, key: KeyEvent) -> bool {
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
    pub(in crate::app) fn inspect_selected_node(&mut self) {
        let Some(node) = self.graph.selected_node() else {
            return;
        };
        let links: Vec<crate::app::trace_json::NodeLink> = self
            .graph
            .links_of(&node.id)
            .iter()
            .map(|(kind, other)| {
                let label = self.graph.node(other).map(|n| n.label.clone());
                (kind.clone(), other.clone(), label)
            })
            .collect();
        let json = crate::app::trace_json::node_json(&with_time(node), &links);
        let title = node_title(node);
        let view = FileView::snapshot(&title, &json, "json", 1);
        self.open_detail(DetailView::File(Box::new(view)));
        self.graph.behind = true;
    }

    /// After a file view closes: back to the graph when it was opened
    /// from there. Returns whether the graph came back.
    pub(in crate::app) fn return_to_graph(&mut self) -> bool {
        if std::mem::take(&mut self.graph.behind) {
            self.open_detail(DetailView::Graph);
            return true;
        }
        false
    }

    /// Label the selected step (or the run, on the run node) good or bad.
    pub(super) fn label_selected(&mut self, value: f64) {
        let Some(node) = self.graph.selected_node() else {
            return;
        };
        let id = node.id.clone();
        let seq = if node.kind == "run" { None } else { node.seq };
        if node.kind != "run" && seq.is_none() {
            self.notifications.current = Some((
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
    pub(super) fn jump_to_record(&mut self) {
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
                self.notifications.current = Some((
                    "No kernel record linked to this node".into(),
                    NotificationType::Warning,
                ));
            }
            (Some(seq), None) => {
                self.notifications.current = Some((
                    format!("Record #{seq} is not in the Record panel (another agent or session)"),
                    NotificationType::Warning,
                ));
            }
            (Some(_), Some(index)) => {
                self.selected_record = index;
                self.records_tab = crate::app::RecordsTab::Kernel;
                self.focus = PanelFocus::Records;
                self.open_detail(DetailView::Record);
            }
        }
    }

    /// Open the run the selected node links to (the child a subagent call
    /// started, or on the run node the run that spawned it), else the
    /// first file the node touched.
    pub(super) fn open_node_file(&mut self) {
        let run = self.graph.selected_node().and_then(|n| {
            let key = if n.kind == "run" {
                "spawned_by"
            } else {
                "child_run"
            };
            meta(n, key).filter(|id| !id.is_empty() && id != "null")
        });
        if let Some(run) = run {
            self.request_graph(&run);
            return;
        }
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
            self.notifications.current = Some((
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

pub(super) fn meta(node: &TraceNode, key: &str) -> Option<String> {
    node.meta
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

/// `node` with a local `time` (to the millisecond) first in its metadata,
/// from when it happened or the run started.
pub(super) fn with_time(node: &TraceNode) -> TraceNode {
    let mut node = node.clone();
    let time = meta(&node, "at_ms")
        .or_else(|| meta(&node, "started_at_ms"))
        .and_then(|ms| ms.parse::<i64>().ok())
        .and_then(chrono::DateTime::from_timestamp_millis);
    if let Some(time) = time {
        let local = time.with_timezone(&chrono::Local);
        let text = local.format("%Y-%m-%d %H:%M:%S%.3f").to_string();
        node.meta.insert(0, ("time".to_string(), text));
    }
    node
}

pub(super) fn numbers(text: &str) -> Vec<u64> {
    text.split(|c: char| !c.is_ascii_digit())
        .filter_map(|s| s.parse().ok())
        .collect()
}
