//! Linked graph of a run for the terminal graph view.
//!
//! Nodes form a containment tree (run → turn → iteration → tool call
//! and decision) plus cross edges: the decision that shaped a model
//! call, tool calls that touched the same file, and rewards on their
//! targets. [`RunGraph::apply`] is incremental so a live run can update
//! the view step by step; loading a finished run replays the same calls.

use std::collections::HashMap;

use aura_core_types::trace::{HarnessAction, Step, ToolIntent};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::bundle::RunMeta;
use crate::recorder::excerpt;

mod annotate;
mod failed;
mod step;

/// Kind of a graph node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// The whole run.
    Run,
    /// One user turn.
    Turn,
    /// One model call.
    Iteration,
    /// One tool call and its result.
    ToolCall,
    /// One harness decision.
    Decision,
    /// A stored reward (outcome or human label).
    Reward,
    /// A child agent run.
    Subagent,
    /// Where a branched run diverged from its parent.
    Branch,
}

/// Kind of a graph edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Parent contains child.
    Contains,
    /// Timeline order between siblings.
    Next,
    /// A decision shaped the target step.
    Triggered,
    /// Two tool calls touched the same file.
    Touched,
    /// A tool call started a child agent.
    Spawned,
    /// A branched run diverged here.
    BranchedFrom,
    /// A reward scores its target.
    Rewards,
}

/// Whether a node succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    /// Neutral or still running.
    #[default]
    Pending,
    /// Finished normally.
    Ok,
    /// Harness intervened (nudge, block, warning).
    Warn,
    /// Failed.
    Error,
}

/// One node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    /// Stable id (`run`, `t3`, `s42`, `tc:<tool_use_id>`, `r7`).
    pub id: String,
    /// Kind.
    pub kind: NodeKind,
    /// Containing node.
    pub parent: Option<String>,
    /// Short label for the tree row.
    pub label: String,
    /// Right-aligned summary (tokens, status, duration).
    pub detail: String,
    /// Success state.
    pub status: NodeStatus,
    /// Step that produced the node, when there is one.
    pub seq: Option<u64>,
    /// Every field shown in the metadata pane.
    pub meta: Map<String, Value>,
    /// Ids of child nodes in order.
    pub children: Vec<String>,
}

/// One edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    /// Source node id.
    pub from: String,
    /// Target node id.
    pub to: String,
    /// Kind.
    pub kind: EdgeKind,
}

/// The run graph.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunGraph {
    /// Nodes in insertion order.
    pub nodes: Vec<GraphNode>,
    /// Edges.
    pub edges: Vec<GraphEdge>,
    #[serde(skip)]
    index: HashMap<String, usize>,
    #[serde(skip)]
    current_turn: Option<String>,
    #[serde(skip)]
    last_iteration: Option<String>,
    #[serde(skip)]
    by_seq: HashMap<u64, String>,
    #[serde(skip)]
    last_touch: HashMap<String, String>,
    /// Policy decisions that arrived before their tool call's node.
    #[serde(skip)]
    pending_policy: HashMap<String, Value>,
}

impl RunGraph {
    /// Graph holding only the run node for `meta`.
    #[must_use]
    pub fn new(meta: &RunMeta) -> Self {
        let mut graph = Self::default();
        let mut node_meta = Map::new();
        node_meta.insert("run_id".into(), json!(meta.run_id));
        node_meta.insert("model".into(), json!(meta.model));
        node_meta.insert("source".into(), json!(meta.source.as_str()));
        node_meta.insert("harness".into(), json!(meta.harness_version));
        node_meta.insert("trace_level".into(), json!(meta.trace_level));
        node_meta.insert("params".into(), json!(meta.params));
        node_meta.insert("title".into(), json!(meta.title));
        node_meta.insert("started_at_ms".into(), json!(meta.started_at_ms));
        node_meta.insert("ended_at_ms".into(), json!(meta.ended_at_ms));
        node_meta.insert("mode".into(), json!(meta.mode));
        node_meta.insert("workspace".into(), json!(meta.workspace));
        node_meta.insert("agent_id".into(), json!(meta.agent_id));
        node_meta.insert("spawned_by".into(), json!(meta.spawned_by));
        node_meta.insert("task_id".into(), json!(meta.task_id));
        node_meta.insert("arm".into(), json!(meta.arm));
        node_meta.insert("arm_propensity".into(), json!(meta.arm_propensity));
        node_meta.insert("consent".into(), json!(meta.consent));
        graph.insert(GraphNode {
            id: "run".into(),
            kind: NodeKind::Run,
            parent: None,
            label: format!("RUN {}  {}", short(&meta.run_id), meta.title),
            detail: meta.model.clone(),
            status: NodeStatus::Pending,
            seq: None,
            meta: node_meta,
            children: Vec::new(),
        });
        if let (Some(parent), Some(step)) = (&meta.parent_run, meta.branch_step) {
            let mut m = Map::new();
            m.insert("parent_run".into(), json!(parent));
            m.insert("branch_step".into(), json!(step));
            graph.insert(GraphNode {
                id: "branch".into(),
                kind: NodeKind::Branch,
                parent: Some("run".into()),
                label: format!("branched from {} after step {step}", short(parent)),
                detail: String::new(),
                status: NodeStatus::Pending,
                seq: None,
                meta: m,
                children: Vec::new(),
            });
            graph.edge("branch", "run", EdgeKind::BranchedFrom);
        }
        graph
    }

    /// Node by id.
    #[must_use]
    pub fn node(&self, id: &str) -> Option<&GraphNode> {
        self.index.get(id).map(|&i| &self.nodes[i])
    }

    /// Id of the node created for step `seq`.
    #[must_use]
    pub fn node_for_seq(&self, seq: u64) -> Option<&str> {
        self.by_seq.get(&seq).map(String::as_str)
    }

    /// Nodes that changed since the graph had `before` nodes and step
    /// `seq` was applied: the new nodes, the step's node and their
    /// ancestors, parents before children.
    #[must_use]
    pub fn changed_since(&self, before: usize, seq: u64) -> Vec<&GraphNode> {
        let mut ids: Vec<&str> = self.nodes[before.min(self.nodes.len())..]
            .iter()
            .map(|n| n.id.as_str())
            .collect();
        if let Some(id) = self.node_for_seq(seq) {
            ids.push(id);
        }
        let mut out: Vec<&GraphNode> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for id in ids {
            let mut chain = Vec::new();
            let mut cursor = self.node(id);
            while let Some(node) = cursor {
                chain.push(node);
                cursor = node.parent.as_deref().and_then(|p| self.node(p));
            }
            for node in chain.into_iter().rev() {
                if seen.insert(node.id.as_str()) {
                    out.push(node);
                }
            }
        }
        out
    }

    /// Edges touching `id`, in either direction, excluding containment.
    #[must_use]
    pub fn links(&self, id: &str) -> Vec<&GraphEdge> {
        self.edges
            .iter()
            .filter(|e| e.kind != EdgeKind::Contains && (e.from == id || e.to == id))
            .collect()
    }

    fn node_mut(&mut self, id: &str) -> Option<&mut GraphNode> {
        let i = *self.index.get(id)?;
        Some(&mut self.nodes[i])
    }

    fn insert(&mut self, node: GraphNode) {
        if self.index.contains_key(&node.id) {
            return;
        }
        if let Some(parent) = node.parent.clone() {
            if let Some(p) = self.node_mut(&parent) {
                p.children.push(node.id.clone());
            }
            self.edges.push(GraphEdge {
                from: parent,
                to: node.id.clone(),
                kind: EdgeKind::Contains,
            });
        }
        if let Some(seq) = node.seq {
            self.by_seq.insert(seq, node.id.clone());
        }
        self.index.insert(node.id.clone(), self.nodes.len());
        self.nodes.push(node);
    }

    fn edge(&mut self, from: &str, to: &str, kind: EdgeKind) {
        let edge = GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            kind,
        };
        if !self.edges.contains(&edge) {
            self.edges.push(edge);
        }
    }

    fn turn_parent(&mut self, step: &Step) -> String {
        if let Some(t) = &self.current_turn {
            return t.clone();
        }
        let id = format!("t{}", step.turn);
        let mut meta = Map::new();
        meta.insert("at_ms".into(), json!(step.at_ms));
        self.insert(GraphNode {
            id: id.clone(),
            kind: NodeKind::Turn,
            parent: Some("run".into()),
            label: format!("T{}", step.turn),
            detail: String::new(),
            status: NodeStatus::Pending,
            seq: None,
            meta,
            children: Vec::new(),
        });
        self.current_turn = Some(id.clone());
        id
    }
}

fn decision_detail(action: &HarnessAction) -> String {
    match action {
        HarnessAction::Nudge { kind } => format!("nudge {kind}"),
        HarnessAction::Block { tool, reason, .. } => {
            format!("block {tool} {}", excerpt(reason, 40))
        }
        HarnessAction::Allow => "allow".into(),
        HarnessAction::Compact {
            tier,
            tokens_before,
            tokens_after,
        } => format!(
            "{tier} {}→{}",
            tokens(*tokens_before),
            tokens(*tokens_after)
        ),
        HarnessAction::CacheHit { tool, .. } => format!("cache {tool}"),
        HarnessAction::Retry {
            attempt, reason, ..
        } => format!("retry #{attempt} {reason}"),
        HarnessAction::Warn { message } => format!("warn {}", excerpt(message, 40)),
        HarnessAction::Build {
            success,
            error_count,
        } => {
            if *success {
                "build ok".into()
            } else {
                format!("build fail {error_count}")
            }
        }
        HarnessAction::Stop { reason } => format!("stop {reason}"),
        HarnessAction::Route { model, effort } => match effort {
            Some(effort) => format!("route {model} {effort}"),
            None => format!("route {model}"),
        },
    }
}

fn intent_mix(intents: impl Iterator<Item = ToolIntent>) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for i in intents {
        match counts.iter_mut().find(|(n, _)| n == i.as_str()) {
            Some((_, c)) => *c += 1,
            None => counts.push((i.as_str().to_string(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    counts
}

fn args_summary(input: &Value) -> String {
    for key in [
        "path",
        "command",
        "query",
        "pattern",
        "description",
        "prompt",
    ] {
        if let Some(v) = input.get(key).and_then(Value::as_str) {
            return excerpt(first_line(v), 50);
        }
    }
    String::new()
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or_default()
}

fn short(id: &str) -> &str {
    id.rsplit('-').next().unwrap_or(id)
}

/// `12.4k`-style token count.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// `3.1s`-style duration.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn secs(ms: u64) -> String {
    if ms >= 60_000 {
        format!("{}m{:02}s", ms / 60_000, (ms % 60_000) / 1000)
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

#[cfg(test)]
mod tests;
