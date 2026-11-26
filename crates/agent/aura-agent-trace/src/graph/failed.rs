//! Graph bookkeeping shared by the step handlers.

use aura_core_types::trace::Step;
use serde_json::{json, Map, Value};

use super::{first_line, EdgeKind, GraphNode, NodeKind, NodeStatus, RunGraph};
use aura_core_types::text::truncate_with_ellipsis;

impl RunGraph {
    pub(super) fn failed_call(&mut self, turn: &str, step: &Step, error: &str) {
        let calls = self.node(turn).map_or(0, |t| {
            t.children
                .iter()
                .filter(|c| {
                    self.node(c).is_some_and(|n| {
                        n.kind == NodeKind::Iteration && !n.meta.contains_key("purpose")
                    })
                })
                .count()
        });
        let id = format!("s{}:failed", step.seq);
        let mut meta = Map::new();
        meta.insert("at_ms".into(), json!(step.at_ms));
        meta.insert("error".into(), json!(error));
        self.insert(GraphNode {
            id: id.clone(),
            kind: NodeKind::Iteration,
            parent: Some(turn.to_string()),
            label: format!("I{} failed", calls + 1),
            detail: truncate_with_ellipsis(first_line(error), 60),
            status: NodeStatus::Error,
            seq: None,
            meta,
            children: Vec::new(),
        });
        if let Some(prev) = self.last_iteration.replace(id.clone()) {
            self.edge(&prev, &id, EdgeKind::Next);
        }
    }

    /// Summed cost of the model calls `keep` selects; `None` when none
    /// has a known price.
    pub(super) fn cost_of(&self, keep: impl Fn(&GraphNode) -> bool) -> Option<f64> {
        self.nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Iteration && keep(n))
            .filter_map(|n| n.meta.get("cost_usd").and_then(Value::as_f64))
            .fold(None, |sum, c| Some(sum.unwrap_or(0.0) + c))
    }
}
