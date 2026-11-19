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

use aura_model::model_catalog::{call_tokens, estimate_cost_usd};
use aura_model::Usage;

use crate::bundle::RunMeta;
use crate::recorder::excerpt;
use crate::records::RecordRow;

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
                meta.insert("at_ms".into(), json!(step.at_ms));
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
                let billed = Usage {
                    input_tokens: usage.input,
                    output_tokens: usage.output,
                    cache_creation_input_tokens: Some(usage.cache_write),
                    cache_read_input_tokens: Some(usage.cache_read),
                };
                let total = call_tokens(&step.policy.id, &billed);
                let prompt_tokens = total.saturating_sub(usage.output);
                let cost = estimate_cost_usd(&step.policy.id, &billed);
                let truncated = action.stop_reason == "max_tokens";
                let intents = intent_mix(action.tool_calls.iter().map(|c| c.intent));
                let mut meta = Map::new();
                meta.insert("seq".into(), json!(step.seq));
                meta.insert("at_ms".into(), json!(step.at_ms));
                meta.insert("iteration".into(), json!(step.iteration));
                meta.insert("model".into(), json!(step.policy.id));
                meta.insert("params".into(), step.policy.params.clone());
                meta.insert("usage".into(), json!(usage));
                meta.insert("prompt_tokens".into(), json!(prompt_tokens));
                meta.insert("cost_usd".into(), json!(cost));
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
                        "{}/{}  {}  {}{}",
                        tokens(prompt_tokens),
                        tokens(usage.output),
                        secs(step.duration_ms.unwrap_or_default()),
                        intents.first().map_or("answer", |(i, _)| i.as_str()),
                        if truncated { "  max tokens" } else { "" }
                    ),
                    status: if truncated {
                        NodeStatus::Warn
                    } else {
                        NodeStatus::Ok
                    },
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
                    m.insert("at_ms".into(), json!(step.at_ms));
                    let call_id = format!("tc:{}", call.tool_use_id);
                    self.insert(GraphNode {
                        id: call_id.clone(),
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
                    if let Some(policy) = self.pending_policy.remove(&call_id) {
                        self.set_policy(&call_id, policy);
                    }
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
                    if let Some(policy) = self.pending_policy.remove(&id) {
                        self.set_policy(&id, policy);
                    }
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
                    node.meta.insert("ended_at_ms".into(), json!(step.at_ms));
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
                meta.insert("at_ms".into(), json!(step.at_ms));
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
                error,
            } => {
                let turn = self.turn_parent(step);
                self.by_seq.insert(step.seq, turn.clone());
                if let Some(error) = error {
                    self.failed_call(&turn, step, error);
                }
                let cost = self.cost_of(|n| n.parent.as_deref() == Some(turn.as_str()));
                if let Some(node) = self.node_mut(&turn) {
                    node.status = if matches!(stop_reason.as_str(), "end_turn" | "task_done") {
                        NodeStatus::Ok
                    } else {
                        NodeStatus::Error
                    };
                    node.detail = format!(
                        "{stop_reason}  {}  {iterations} calls  {files_changed} files{}",
                        tokens(input_tokens + output_tokens),
                        cost.map(|c| format!("  ${c:.4}")).unwrap_or_default()
                    );
                    let started = node.meta.get("at_ms").and_then(Value::as_u64);
                    node.meta.insert("stop_reason".into(), json!(stop_reason));
                    node.meta.insert("error".into(), json!(error));
                    node.meta.insert("iterations".into(), json!(iterations));
                    node.meta.insert("input_tokens".into(), json!(input_tokens));
                    node.meta
                        .insert("output_tokens".into(), json!(output_tokens));
                    node.meta.insert("cost_usd".into(), json!(cost));
                    node.meta.insert("ended_at_ms".into(), json!(step.at_ms));
                    node.meta.insert(
                        "duration_ms".into(),
                        json!(started.map(|s| step.at_ms.saturating_sub(s))),
                    );
                    node.meta.insert("files".into(), json!(step.links.files));
                }
                let run_cost = self.cost_of(|_| true);
                if let Some(run) = self.node_mut("run") {
                    run.meta.insert("cost_usd".into(), json!(run_cost));
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

    /// Add a kernel record row: a policy decision goes on its tool call.
    /// Returns the id of the node that changed.
    pub fn apply_record(&mut self, row: &RecordRow) -> Option<String> {
        if row.tx_type != "policy_decision" {
            return None;
        }
        let id = format!("tc:{}", row.tool_use_id.as_deref()?);
        let policy = json!({
            "verdict": row.verdict,
            "rule": row.rule,
            "approver": row.approver,
            "reason": row.reason,
            "record_seq": row.seq,
            "at_ms": row.at_ms,
        });
        if self.node(&id).is_none() {
            self.pending_policy.insert(id, policy);
            return None;
        }
        self.set_policy(&id, policy);
        Some(id)
    }

    fn set_policy(&mut self, id: &str, policy: Value) {
        let Some(node) = self.node_mut(id) else {
            return;
        };
        if node.status == NodeStatus::Pending {
            match policy.get("verdict").and_then(Value::as_str) {
                Some("denied") => {
                    node.status = NodeStatus::Error;
                    node.detail = "denied".into();
                }
                Some("pending_approval") => node.detail = "awaiting approval".into(),
                _ if node.detail == "awaiting approval" => node.detail = "running".into(),
                _ => {}
            }
        }
        node.meta.insert("policy".into(), policy);
    }

    /// Link runs of agents this run spawned to the calls that spawned
    /// them: the call whose prompt the child's title was taken from, else
    /// the latest call made before the child started (a retry). Returns
    /// the ids of the nodes that changed.
    pub fn link_child_runs(&mut self, children: &[RunMeta]) -> Vec<String> {
        let mut children: Vec<&RunMeta> = children.iter().collect();
        children.sort_by_key(|c| c.started_at_ms);
        let mut changed: Vec<String> = Vec::new();
        for child in children {
            let lists = |n: &GraphNode| {
                n.meta
                    .get("child_runs")
                    .and_then(Value::as_array)
                    .is_some_and(|l| l.iter().any(|v| v.as_str() == Some(&child.run_id)))
            };
            if self.nodes.iter().any(|n| n.id != "run" && lists(n)) {
                continue;
            }
            let before = |n: &&GraphNode| {
                n.kind == NodeKind::Subagent
                    && n.meta
                        .get("at_ms")
                        .and_then(Value::as_u64)
                        .is_none_or(|at| at <= child.started_at_ms)
            };
            let titled = |n: &&GraphNode| {
                !n.meta.contains_key("child_run")
                    && n.meta
                        .get("input")
                        .and_then(|i| i.get("prompt"))
                        .and_then(Value::as_str)
                        .is_some_and(|p| excerpt(first_line(p), 80) == child.title)
            };
            let target = self
                .nodes
                .iter()
                .filter(before)
                .find(titled)
                .or_else(|| self.nodes.iter().rev().find(before))
                .map(|n| n.id.clone());
            for id in std::iter::once("run".to_string()).chain(target) {
                let Some(node) = self.node_mut(&id) else {
                    continue;
                };
                if id != "run" {
                    node.meta
                        .entry("child_run")
                        .or_insert_with(|| json!(child.run_id));
                }
                let list = node
                    .meta
                    .entry("child_runs")
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let Value::Array(list) = list {
                    if !list.iter().any(|v| v.as_str() == Some(&child.run_id)) {
                        list.push(json!(child.run_id));
                    }
                }
                if !changed.contains(&id) {
                    changed.push(id);
                }
            }
        }
        changed
    }

    fn failed_call(&mut self, turn: &str, step: &Step, error: &str) {
        let calls = self.node(turn).map_or(0, |t| {
            t.children
                .iter()
                .filter(|c| self.node(c).is_some_and(|n| n.kind == NodeKind::Iteration))
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
            detail: excerpt(first_line(error), 60),
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
    fn cost_of(&self, keep: impl Fn(&GraphNode) -> bool) -> Option<f64> {
        self.nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Iteration && keep(n))
            .filter_map(|n| n.meta.get("cost_usd").and_then(Value::as_f64))
            .fold(None, |sum, c| Some(sum.unwrap_or(0.0) + c))
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

    fn turn_end(seq: u64, stop_reason: &str, error: Option<&str>) -> Step {
        base(
            seq,
            Actor::Environment,
            StepAction::TurnEnd {
                stop_reason: stop_reason.into(),
                iterations: 1,
                input_tokens: 0,
                output_tokens: 0,
                files_changed: 0,
                error: error.map(str::to_string),
            },
        )
    }

    fn policy(seq: u64, id: &str, verdict: &str) -> RecordRow {
        RecordRow {
            seq,
            tx_type: "policy_decision".into(),
            at_ms: seq * 1000,
            tool_use_id: Some(id.into()),
            tool: Some("edit_file".into()),
            verdict: Some(verdict.into()),
            approver: Some("user".into()),
            rule: Some("mode.ask".into()),
            reason: None,
        }
    }

    #[test]
    fn model_calls_are_timed_priced_and_flag_max_tokens() {
        let mut m = model(2, &[]);
        m.policy.id = "claude-opus-4-5".into();
        m.usage = Some(aura_core_types::trace::TokenUsage {
            input: 200,
            output: 100,
            cache_write: 0,
            cache_read: 800,
        });
        if let StepAction::Model(action) = &mut m.action {
            action.stop_reason = "max_tokens".into();
        }
        let mut g = RunGraph::new(&RunMeta::new("m", RunSource::Test));
        g.apply(&m);
        g.apply(&turn_end(3, "end_turn", None));
        let call = g.node("s2").unwrap();
        assert_eq!(call.status, NodeStatus::Warn);
        assert!(call.detail.starts_with("1.0k/100"), "{}", call.detail);
        assert!(call.detail.ends_with("max tokens"));
        assert_eq!(call.meta["at_ms"], json!(2000));
        assert_eq!(call.meta["prompt_tokens"], json!(1000));
        let cost = call.meta["cost_usd"].as_f64().unwrap();
        assert!((cost - 0.0039).abs() < 1e-9);
        let turn = g.node("t1").unwrap();
        assert_eq!(turn.meta["cost_usd"], json!(cost));
        assert_eq!(turn.meta["duration_ms"], json!(1000));
        assert_eq!(g.node("run").unwrap().meta["cost_usd"], json!(cost));
    }

    #[test]
    fn a_turn_that_ended_on_an_error_shows_the_failed_call() {
        let mut g = RunGraph::new(&RunMeta::new("m", RunSource::Test));
        g.apply(&model(1, &[]));
        g.apply(&turn_end(2, "llm_error", Some("overloaded\nretry later")));
        let failed = g.node("s2:failed").unwrap();
        assert_eq!(failed.label, "I2 failed");
        assert_eq!(failed.detail, "overloaded");
        assert_eq!(failed.status, NodeStatus::Error);
        assert!(g.links("s1").iter().any(|e| e.to == "s2:failed"));
        assert_eq!(
            g.node("t1").unwrap().meta["error"],
            json!("overloaded\nretry later")
        );
    }

    #[test]
    fn policy_decisions_land_on_their_tool_call_in_either_order() {
        let mut g = RunGraph::new(&RunMeta::new("m", RunSource::Test));
        assert_eq!(g.apply_record(&policy(1, "early", "denied")), None);
        g.apply(&model(2, &[("early", "edit_file", json!({"path": "x"}))]));
        let early = g.node("tc:early").unwrap();
        assert_eq!(early.status, NodeStatus::Error);
        assert_eq!(early.meta["policy"]["verdict"], json!("denied"));

        g.apply(&model(3, &[("late", "edit_file", json!({"path": "y"}))]));
        let changed = g.apply_record(&policy(4, "late", "pending_approval"));
        assert_eq!(changed.as_deref(), Some("tc:late"));
        assert_eq!(g.node("tc:late").unwrap().detail, "awaiting approval");
        g.apply_record(&policy(5, "late", "approved"));
        let late = g.node("tc:late").unwrap();
        assert_eq!(late.detail, "running");
        assert_eq!(late.meta["policy"]["approver"], json!("user"));
        assert_eq!(late.meta["policy"]["rule"], json!("mode.ask"));
        assert_eq!(late.meta["policy"]["record_seq"], json!(5));
    }

    #[test]
    fn child_runs_link_to_the_call_that_spawned_them() {
        let parent = RunMeta::new("m", RunSource::Test);
        let mut g = RunGraph::new(&parent);
        g.apply(&model(
            1,
            &[
                ("a", "task", json!({"prompt": "count files\nin src"})),
                ("b", "task", json!({"prompt": "list tests"})),
            ],
        ));
        assert_eq!(g.node("tc:a").unwrap().kind, NodeKind::Subagent);
        let child = |id: &str, title: &str, at: u64| {
            let mut meta = RunMeta::new("m", RunSource::Test);
            meta.run_id = id.into();
            meta.title = title.into();
            meta.started_at_ms = at;
            meta.spawned_by = Some(parent.run_id.clone());
            meta
        };
        let changed = g.link_child_runs(&[
            child("c2", "list tests", 1500),
            child("c1", "count files", 1200),
            child("c3", "list tests, retried", 1800),
        ]);
        assert_eq!(changed, vec!["run", "tc:a", "tc:b"]);
        assert_eq!(g.node("tc:a").unwrap().meta["child_run"], json!("c1"));
        assert_eq!(g.node("tc:b").unwrap().meta["child_run"], json!("c2"));
        assert_eq!(
            g.node("tc:b").unwrap().meta["child_runs"],
            json!(["c2", "c3"])
        );
        assert_eq!(
            g.node("run").unwrap().meta["child_runs"],
            json!(["c1", "c2", "c3"])
        );
        assert!(g
            .link_child_runs(&[child("c1", "count files", 1200)])
            .is_empty());
    }

    #[test]
    fn the_run_node_shows_unset_fields_as_null() {
        let g = RunGraph::new(&RunMeta::new("m", RunSource::Test));
        let run = &g.node("run").unwrap().meta;
        assert_eq!(run["ended_at_ms"], Value::Null);
        assert_eq!(run["spawned_by"], Value::Null);
        assert!(run["started_at_ms"].as_u64().is_some());
    }
}
