//! Annotations from outside the step stream: rewards, kernel policy
//! records and links to child runs.

use aura_core_types::trace::{Reward, RewardKind, RewardTarget};
use serde_json::{json, Map, Value};

use super::{first_line, EdgeKind, GraphNode, NodeKind, NodeStatus, RunGraph};
use crate::bundle::RunMeta;
use crate::recorder::excerpt;
use crate::records::RecordRow;

impl RunGraph {
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
            self.add_reward_node(reward, &target);
        }
    }

    /// Outcome and human rewards get a node of their own.
    fn add_reward_node(&mut self, reward: &Reward, target: &str) {
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
        self.edge(&id, target, EdgeKind::Rewards);
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

    pub(super) fn set_policy(&mut self, id: &str, policy: Value) {
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
            let target = self.spawning_call(child);
            self.mark_child_run(child, target, &mut changed);
        }
        changed
    }

    /// The call that spawned `child`: the one whose prompt its title came
    /// from, else the latest made before it started.
    fn spawning_call(&self, child: &RunMeta) -> Option<String> {
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
        self.nodes
            .iter()
            .filter(before)
            .find(titled)
            .or_else(|| self.nodes.iter().rev().find(before))
            .map(|n| n.id.clone())
    }

    /// Record `child` on the run node and on its spawning call.
    fn mark_child_run(
        &mut self,
        child: &RunMeta,
        target: Option<String>,
        changed: &mut Vec<String>,
    ) {
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
}
