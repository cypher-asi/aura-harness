//! Per-format record builders: chat transcripts, RL transitions, harness
//! bandit decisions and preference pairs.

use std::collections::{BTreeMap, HashMap};

use aura_core_types::trace::{Actor, RewardKind, RewardTarget, Step, StepAction};
use aura_store_snapshot::{FsSnapshotStore, SnapshotStore};
use serde_json::{json, Value};

use super::ExportFormat;
use crate::normalize::{RunTrace, DEFAULT_GAMMA};

pub(super) fn task_key(trace: &RunTrace) -> String {
    trace
        .meta
        .task_id
        .clone()
        .or_else(|| trace.meta.parent_run.clone())
        .unwrap_or_else(|| {
            if trace.meta.title.is_empty() {
                trace.meta.run_id.clone()
            } else {
                trace.meta.title.to_lowercase()
            }
        })
}

/// Chat transcript of a run. Tool outputs come from blobs when the run
/// was recorded at `full`, otherwise from the stored excerpt.
pub(super) fn chat(trace: &RunTrace, blobs: Option<&FsSnapshotStore>) -> Vec<Value> {
    let mut messages = Vec::new();
    for step in &trace.steps {
        match &step.action {
            StepAction::UserPrompt { text } => {
                messages.push(json!({"role": "user", "content": text}));
            }
            StepAction::Model(action) if step.actor == Actor::Model => {
                let tool_calls: Vec<Value> = action
                    .tool_calls
                    .iter()
                    .map(|c| json!({"id": c.tool_use_id, "name": c.name, "arguments": c.input}))
                    .collect();
                let mut msg = json!({"role": "assistant", "content": action.text});
                if !tool_calls.is_empty() {
                    msg["tool_calls"] = Value::Array(tool_calls);
                }
                messages.push(msg);
            }
            StepAction::ToolResult {
                tool_use_id,
                excerpt,
                content,
                is_error,
                ..
            } => {
                let full = content
                    .as_ref()
                    .zip(blobs)
                    .and_then(|(id, store)| store.get(id).ok().flatten())
                    .and_then(|b| String::from_utf8(b.to_vec()).ok());
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": full.unwrap_or_else(|| excerpt.clone()),
                    "is_error": is_error,
                }));
            }
            StepAction::Model(_) | StepAction::Harness { .. } | StepAction::TurnEnd { .. } => {}
        }
    }
    messages
}

pub(super) fn records_for(
    format: ExportFormat,
    trace: &RunTrace,
    blobs: Option<&FsSnapshotStore>,
) -> Vec<Value> {
    match format {
        ExportFormat::Sft => vec![json!({
            "run_id": &trace.meta.run_id,
            "model": trace.meta.model,
            "outcome": trace.outcome(),
            "messages": chat(trace, blobs),
        })],
        ExportFormat::Rl => rl_records(trace),
        ExportFormat::HarnessBandit => bandit_records(trace),
        ExportFormat::Preference => Vec::new(),
    }
}

/// One transition per step with its reward and discounted return.
fn rl_records(trace: &RunTrace) -> Vec<Value> {
    let run_id = &trace.meta.run_id;
    let returns: HashMap<u64, (f64, f64)> = trace
        .returns(DEFAULT_GAMMA)
        .into_iter()
        .map(|(seq, r, g)| (seq, (r, g)))
        .collect();
    let last = trace.steps.last().map(|s| s.seq);
    trace
        .steps
        .iter()
        .map(|s| {
            let (reward, ret) = returns.get(&s.seq).copied().unwrap_or_default();
            json!({
                "run_id": run_id,
                "seq": s.seq,
                "turn": s.turn,
                "actor": s.actor,
                "observation": s.observation,
                "action": s.action,
                "reward": reward,
                "return": ret,
                "done": Some(s.seq) == last,
                "policy": s.policy,
                "propensity": s.policy.propensity,
            })
        })
        .collect()
}

/// One record per harness decision with its context and measured effect.
fn bandit_records(trace: &RunTrace) -> Vec<Value> {
    let effects: HashMap<u64, f64> = trace
        .derived_rewards
        .iter()
        .filter(|r| r.kind == RewardKind::DecisionEffect)
        .filter_map(|r| match r.target {
            RewardTarget::Step { seq } => Some((seq, r.value)),
            RewardTarget::Run => None,
        })
        .collect();
    trace
        .steps
        .iter()
        .filter_map(|s| bandit_record(trace, s, &effects))
        .collect()
}

fn bandit_record(trace: &RunTrace, s: &Step, effects: &HashMap<u64, f64>) -> Option<Value> {
    let Actor::Harness { source } = s.actor else {
        return None;
    };
    let StepAction::Harness {
        name,
        action,
        alternatives,
    } = &s.action
    else {
        return None;
    };
    let (features, trigger) = match &s.observation {
        aura_core_types::trace::Observation::Harness { features, trigger } => {
            (features.clone(), trigger.clone())
        }
        _ => (Value::Null, Value::Null),
    };
    Some(json!({
        "run_id": &trace.meta.run_id,
        "seq": s.seq,
        "source": source,
        "name": name,
        "version": s.policy.version,
        "features": features,
        "trigger": trigger,
        "action": action,
        "alternatives": alternatives,
        "params": s.policy.params,
        "propensity": s.policy.propensity,
        "effect": effects.get(&s.seq),
        "outcome": trace.outcome(),
    }))
}

pub(super) fn preference_pairs(
    traces: &[&RunTrace],
    blobs: Option<&FsSnapshotStore>,
) -> Vec<(String, Value)> {
    let mut by_task: BTreeMap<String, Vec<&RunTrace>> = BTreeMap::new();
    for t in traces {
        if t.outcome().is_some() {
            by_task.entry(task_key(t)).or_default().push(t);
        }
    }
    let mut out = Vec::new();
    for (task, mut runs) in by_task {
        runs.sort_by(|a, b| {
            b.outcome()
                .partial_cmp(&a.outcome())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let (Some(best), Some(worst)) = (runs.first(), runs.last()) else {
            continue;
        };
        if best.outcome() <= worst.outcome() {
            continue;
        }
        let shared_prefix = match (&worst.meta.parent_run, worst.meta.branch_step) {
            (Some(p), Some(step)) if *p == best.meta.run_id => Some(step),
            _ => match (&best.meta.parent_run, best.meta.branch_step) {
                (Some(p), Some(step)) if *p == worst.meta.run_id => Some(step),
                _ => None,
            },
        };
        out.push((
            task.clone(),
            json!({
                "task": task,
                "chosen_run": best.meta.run_id,
                "rejected_run": worst.meta.run_id,
                "chosen_outcome": best.outcome(),
                "rejected_outcome": worst.outcome(),
                "shared_prefix_steps": shared_prefix,
                "chosen": chat(best, blobs),
                "rejected": chat(worst, blobs),
            }),
        ));
    }
    out
}
