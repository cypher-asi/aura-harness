//! Linked graph of a run for the terminal graph view.
//!
//! Nodes form a containment tree (run → turn → iteration → tool call
//! and decision) plus cross edges: the decision that shaped a model
//! call, tool calls that touched the same file, and rewards on their
//! targets. [`RunGraph::apply`] is incremental so a live run can update
//! the view step by step; loading a finished run replays the same calls.

use std::collections::HashMap;

use aura_core_types::trace::{
    Actor, HarnessAction, Reward, RewardKind, RewardTarget, Step, StepAction, ToolIntent,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::bundle::RunMeta;
use crate::recorder::excerpt;

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
        if let Some(task) = &meta.task_id {
            node_meta.insert("task_id".into(), json!(task));
        }
        if let Some(arm) = &meta.arm {
            node_meta.insert("arm".into(), json!(arm));
        }
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
        self.insert(GraphNode {
            id: id.clone(),
            kind: NodeKind::Turn,
            parent: Some("run".into()),
            label: format!("T{}", step.turn),
            detail: String::new(),
            status: NodeStatus::Pending,
            seq: None,
            meta: Map::new(),
            children: Vec::new(),
        });
        self.current_turn = Some(id.clone());
        id
    }

    /// Add one step.
    pub fn apply(&mut self, step: &Step) {
        self.apply_action(step);
        if step.links.record_seqs.is_empty() {
            return;
        }
        if let Some(id) = self.by_seq.get(&step.seq).cloned() {
            if let Some(node) = self.node_mut(&id) {
                node.meta
                    .insert("record_seqs".into(), json!(step.links.record_seqs));
            }
        }
    }

    fn apply_action(&mut self, step: &Step) {
        match &step.action {
            StepAction::UserPrompt { text } => {
                let id = format!("t{}", step.turn);
                let mut meta = Map::new();
                meta.insert("prompt".into(), json!(text));
                self.insert(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Turn,
                    parent: Some("run".into()),
                    label: format!("T{} \"{}\"", step.turn, excerpt(first_line(text), 60)),
                    detail: String::new(),
                    status: NodeStatus::Pending,
                    seq: Some(step.seq),
                    meta,
                    children: Vec::new(),
                });
                self.current_turn = Some(id);
                self.last_iteration = None;
            }
            StepAction::Model(action) => {
                let parent = self.turn_parent(step);
                let id = format!("s{}", step.seq);
                let usage = step.usage.unwrap_or_default();
                let intents = intent_mix(action.tool_calls.iter().map(|c| c.intent));
                let mut meta = Map::new();
                meta.insert("seq".into(), json!(step.seq));
                meta.insert("iteration".into(), json!(step.iteration));
                meta.insert("model".into(), json!(step.policy.id));
                meta.insert("params".into(), step.policy.params.clone());
                meta.insert("usage".into(), json!(usage));
                meta.insert("latency_ms".into(), json!(step.duration_ms));
                meta.insert("stop_reason".into(), json!(action.stop_reason));
                meta.insert("thinking_chars".into(), json!(action.thinking_chars));
                meta.insert("text".into(), json!(excerpt(&action.text, 2000)));
                meta.insert("intents".into(), json!(intents));
                if let aura_core_types::trace::Observation::Model(obs) = &step.observation {
                    meta.insert("observation".into(), json!(obs));
                }
                if let Some(blob) = &action.message {
                    meta.insert("message_blob".into(), json!(blob));
                }
                let n = step.iteration.map_or(step.seq, |i| u64::from(i) + 1);
                self.insert(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Iteration,
                    parent: Some(parent),
                    label: format!("I{n} model"),
                    detail: format!(
                        "{}/{}  {}  {}",
                        tokens(usage.input + usage.cache_read + usage.cache_write),
                        tokens(usage.output),
                        secs(step.duration_ms.unwrap_or_default()),
                        intents.first().map_or("answer", |(i, _)| i.as_str())
                    ),
                    status: NodeStatus::Ok,
                    seq: Some(step.seq),
                    meta,
                    children: Vec::new(),
                });
                if let Some(prev) = self.last_iteration.replace(id.clone()) {
                    self.edge(&prev, &id, EdgeKind::Next);
                }
                for cause in &step.links.caused_by {
                    if let Some(from) = self.by_seq.get(cause).cloned() {
                        self.edge(&from, &id, EdgeKind::Triggered);
                    }
                }
                for call in &action.tool_calls {
                    let mut m = Map::new();
                    m.insert("tool_use_id".into(), json!(call.tool_use_id));
                    m.insert("tool".into(), json!(call.name));
                    m.insert("input".into(), call.input.clone());
                    m.insert("intent".into(), json!(call.intent));
                    self.insert(GraphNode {
                        id: format!("tc:{}", call.tool_use_id),
                        kind: if call.intent == ToolIntent::Delegate {
                            NodeKind::Subagent
                        } else {
                            NodeKind::ToolCall
                        },
                        parent: Some(id.clone()),
                        label: format!("{} {}", call.name, args_summary(&call.input)),
                        detail: "running".into(),
                        status: NodeStatus::Pending,
                        seq: None,
                        meta: m,
                        children: Vec::new(),
                    });
                }
            }
            StepAction::ToolResult {
                tool_use_id,
                tool,
                is_error,
                source,
                content_chars,
                excerpt: text,
                content,
                files,
            } => {
                let id = format!("tc:{tool_use_id}");
                if self.node(&id).is_none() {
                    let parent = self
                        .last_iteration
                        .clone()
                        .unwrap_or_else(|| self.turn_parent(step));
                    self.insert(GraphNode {
                        id: id.clone(),
                        kind: NodeKind::ToolCall,
                        parent: Some(parent),
                        label: tool.clone(),
                        detail: String::new(),
                        status: NodeStatus::Pending,
                        seq: None,
                        meta: Map::new(),
                        children: Vec::new(),
                    });
                }
                self.by_seq.insert(step.seq, id.clone());
                let mut flags = vec![if *is_error { "fail" } else { "ok" }.to_string()];
                if source != "executor" {
                    flags.push(source.clone());
                }
                for f in files {
                    if f.kind != "read" && (f.added > 0 || f.removed > 0) {
                        flags.push(format!("+{} -{}", f.added, f.removed));
                    }
                }
                if let Some(ms) = step.duration_ms {
                    flags.push(secs(ms));
                }
                if let Some(node) = self.node_mut(&id) {
                    node.seq = Some(step.seq);
                    node.status = if *is_error {
                        NodeStatus::Error
                    } else if source == "blocked" {
                        NodeStatus::Warn
                    } else {
                        NodeStatus::Ok
                    };
                    node.detail = flags.join("  ");
                    node.meta.insert("result_seq".into(), json!(step.seq));
                    node.meta.insert("is_error".into(), json!(is_error));
                    node.meta.insert("source".into(), json!(source));
                    node.meta
                        .insert("content_chars".into(), json!(content_chars));
                    node.meta.insert("excerpt".into(), json!(text));
                    node.meta.insert("files".into(), json!(files));
                    node.meta
                        .insert("duration_ms".into(), json!(step.duration_ms));
                    if let Some(blob) = content {
                        node.meta.insert("content_blob".into(), json!(blob));
                    }
                }
                for f in files {
                    if let Some(prev) = self.last_touch.insert(f.path.clone(), id.clone()) {
                        if prev != id {
                            self.edge(&prev, &id, EdgeKind::Touched);
                        }
                    }
                }
            }
            StepAction::Harness {
                name,
                action,
                alternatives,
            } => {
                let parent = step
                    .parent
                    .and_then(|p| self.by_seq.get(&p).cloned())
                    .or_else(|| self.current_turn.clone())
                    .unwrap_or_else(|| self.turn_parent(step));
                let id = format!("s{}", step.seq);
                let source = match step.actor {
                    Actor::Harness { source } => source.as_str(),
                    _ => "harness",
                };
                let mut meta = Map::new();
                meta.insert("seq".into(), json!(step.seq));
                meta.insert("source".into(), json!(source));
                meta.insert("name".into(), json!(name));
                meta.insert("action".into(), json!(action));
                meta.insert("alternatives".into(), json!(alternatives));
                meta.insert("policy".into(), json!(step.policy));
                if let aura_core_types::trace::Observation::Harness { features, trigger } =
                    &step.observation
                {
                    meta.insert("features".into(), features.clone());
                    meta.insert("trigger".into(), trigger.clone());
                }
                meta.insert("links".into(), json!(step.links));
                let status = match action {
                    HarnessAction::Allow | HarnessAction::CacheHit { .. } => NodeStatus::Ok,
                    HarnessAction::Build { success: true, .. } => NodeStatus::Ok,
                    HarnessAction::Build { .. } | HarnessAction::Stop { .. } => NodeStatus::Error,
                    _ => NodeStatus::Warn,
                };
                self.insert(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Decision,
                    parent: Some(parent),
                    label: name.clone(),
                    detail: decision_detail(action),
                    status,
                    seq: Some(step.seq),
                    meta,
                    children: Vec::new(),
                });
                for tool_use_id in &step.links.tool_use_ids {
                    let target = format!("tc:{tool_use_id}");
                    if self.node(&target).is_some() {
                        self.edge(&id, &target, EdgeKind::Triggered);
                    }
                }
            }
            StepAction::TurnEnd {
                stop_reason,
                iterations,
                input_tokens,
                output_tokens,
                files_changed,
            } => {
                let turn = self.turn_parent(step);
                self.by_seq.insert(step.seq, turn.clone());
                if let Some(node) = self.node_mut(&turn) {
                    node.status = if matches!(stop_reason.as_str(), "end_turn" | "task_done") {
                        NodeStatus::Ok
                    } else {
                        NodeStatus::Error
                    };
                    node.detail = format!(
                        "{stop_reason}  {}  {iterations} calls  {files_changed} files",
                        tokens(input_tokens + output_tokens)
                    );
                    node.meta.insert("stop_reason".into(), json!(stop_reason));
                    node.meta.insert("iterations".into(), json!(iterations));
                    node.meta.insert("input_tokens".into(), json!(input_tokens));
                    node.meta
                        .insert("output_tokens".into(), json!(output_tokens));
                    node.meta.insert("files".into(), json!(step.links.files));
                }
                self.current_turn = None;
                self.last_iteration = None;
            }
        }
    }

    /// Add a reward. Outcome and human rewards become nodes; derived
    /// rewards are folded into their target's metadata.
    pub fn apply_reward(&mut self, reward: &Reward) {
        let target = match reward.target {
            RewardTarget::Run => "run".to_string(),
            RewardTarget::Step { seq } => match self.by_seq.get(&seq) {
                Some(id) => id.clone(),
                None => return,
            },
        };
        let standalone = matches!(
            reward.kind,
            RewardKind::Outcome | RewardKind::HumanExplicit | RewardKind::HumanImplicit
        ) && reward.name != "outcome_credit";
        if let Some(node) = self.node_mut(&target) {
            let entry = node
                .meta
                .entry("rewards")
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(list) = entry {
                list.push(json!({
                    "kind": reward.kind.as_str(),
                    "name": reward.name,
                    "value": reward.value,
                    "source": reward.source,
                }));
            }
            if reward.kind == RewardKind::DecisionEffect {
                node.detail = format!(
                    "{}  effect {}",
                    node.detail,
                    if reward.value > 0.0 { "+" } else { "-" }
                );
            }
        }
        if standalone {
            let id = format!("r{}", self.nodes.len());
            let mut meta = Map::new();
            meta.insert("reward".into(), json!(reward));
            self.insert(GraphNode {
                id: id.clone(),
                kind: NodeKind::Reward,
                parent: Some("run".into()),
                label: format!("reward {}={:.2}", reward.name, reward.value),
                detail: reward.source.clone(),
                status: if reward.value >= 0.0 {
                    NodeStatus::Ok
                } else {
                    NodeStatus::Error
                },
                seq: None,
                meta,
                children: Vec::new(),
            });
            self.edge(&id, &target, EdgeKind::Rewards);
            if target == "run" && reward.kind == RewardKind::Outcome {
                if let Some(run) = self.node_mut("run") {
                    run.status = if reward.value > 0.0 {
                        NodeStatus::Ok
                    } else {
                        NodeStatus::Error
                    };
                }
            }
        }
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
mod tests {
    use super::*;
    use crate::bundle::RunSource;
    use crate::rewards::tests::{base, model, nudge, result};
    use crate::RunTrace;
    use serde_json::json;

    fn trace() -> RunTrace {
        let prompt = base(
            1,
            Actor::User,
            StepAction::UserPrompt {
                text: "fix the test".into(),
            },
        );
        let mut m2 = model(3, &[("a", "edit_file", json!({"path": "x"}))]);
        m2.links.caused_by = vec![2];
        let steps = vec![
            prompt,
            nudge(2, "implement_now"),
            m2,
            result(4, 3, "a", "edit_file", "x", "modify", false),
            model(5, &[("b", "read_file", json!({"path": "x"}))]),
            result(6, 5, "b", "read_file", "x", "read", false),
        ];
        RunTrace::from_parts(RunMeta::new("m", RunSource::Test), steps, Vec::new())
    }

    #[test]
    fn builds_containment_and_cross_edges() {
        let g = trace().graph();
        let turn = g.node("t1").unwrap();
        assert_eq!(turn.children, vec!["s2", "s3", "s5"]);
        assert_eq!(g.node("s3").unwrap().children, vec!["tc:a"]);
        assert_eq!(g.node("tc:a").unwrap().status, NodeStatus::Ok);
        let links: Vec<(&str, &str, EdgeKind)> = g
            .links("tc:a")
            .iter()
            .map(|e| (e.from.as_str(), e.to.as_str(), e.kind))
            .collect();
        assert!(links.contains(&("tc:a", "tc:b", EdgeKind::Touched)));
        assert!(g
            .links("s2")
            .iter()
            .any(|e| e.to == "s3" && e.kind == EdgeKind::Triggered));
        assert!(g.links("s3").iter().any(|e| e.kind == EdgeKind::Next));
        assert!(g.node("s2").unwrap().detail.contains("effect +"));
    }

    #[test]
    fn incremental_apply_matches_full_build() {
        let t = trace();
        let mut live = RunGraph::new(&t.meta);
        for s in &t.steps {
            live.apply(s);
        }
        let full = t.graph();
        let shape = |g: &RunGraph| -> Vec<(String, Option<String>)> {
            g.nodes
                .iter()
                .filter(|n| n.kind != NodeKind::Reward)
                .map(|n| (n.id.clone(), n.parent.clone()))
                .collect()
        };
        assert_eq!(shape(&live), shape(&full));
        assert_eq!(live.nodes.len(), 7);
    }

    #[test]
    fn changed_since_returns_new_nodes_and_their_ancestors() {
        let t = trace();
        let mut g = RunGraph::new(&t.meta);
        for s in &t.steps[..3] {
            g.apply(s);
        }
        let before = g.nodes.len();
        g.apply(&t.steps[3]);
        let ids: Vec<&str> = g
            .changed_since(before, t.steps[3].seq)
            .iter()
            .map(|n| n.id.as_str())
            .collect();
        assert_eq!(ids, vec!["run", "t1", "s3", "tc:a"]);
    }
}
