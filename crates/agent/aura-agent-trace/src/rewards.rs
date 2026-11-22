//! Reward labelers and credit assignment.
//!
//! Two kinds of rewards exist:
//!
//! - **Stored** rewards come from outside the step log (verification
//!   outcome, human labels, interrupts) and live in `rewards.jsonl`.
//! - **Derived** rewards (process, cost, tokens, decision effect) are pure
//!   functions of the steps. They are recomputed on every load rather
//!   than stored, so improving a labeler relabels every past run. Each
//!   labeler stamps a versioned `source` so exports record which
//!   labeler produced a value.

use std::collections::HashMap;

use aura_core_types::trace::{
    Actor, DecisionSource, HarnessAction, Reward, RewardKind, RewardTarget, Step, StepAction,
    ToolIntent,
};
use serde_json::json;

use crate::scorecard::step_cost_usd;

/// Source tag of process rewards.
pub const PROCESS_SOURCE: &str = "process/v1";
/// Source tag of cost and token rewards.
pub const COST_SOURCE: &str = "cost/v2";
/// Source tag of decision-effect rewards.
pub const EFFECT_SOURCE: &str = "effect/v1";
/// Model calls after a harness decision within which its intended
/// behavior must appear to count as effective.
pub const EFFECT_WINDOW: usize = 3;

fn reward(
    target: RewardTarget,
    kind: RewardKind,
    name: &str,
    value: f64,
    source: &str,
    at_ms: u64,
) -> Reward {
    Reward {
        target,
        kind,
        name: name.to_string(),
        value,
        source: source.to_string(),
        at_ms,
        evidence: serde_json::Value::Null,
    }
}

fn step_target(step: &Step) -> RewardTarget {
    RewardTarget::Step { seq: step.seq }
}

/// Intent of the tool call a result answers, looked up through the
/// model step that requested it.
fn intents_by_tool_use(steps: &[Step]) -> HashMap<String, (ToolIntent, serde_json::Value)> {
    let mut out = HashMap::new();
    for step in steps {
        if let StepAction::Model(action) = &step.action {
            for call in &action.tool_calls {
                out.insert(call.tool_use_id.clone(), (call.intent, call.input.clone()));
            }
        }
    }
    out
}

/// Every derived reward for `steps`.
#[must_use]
pub fn derive(steps: &[Step]) -> Vec<Reward> {
    let mut out = process_rewards(steps);
    out.extend(cost_rewards(steps));
    out.extend(decision_effects(steps));
    out
}

/// Per-step progress signals from tool results and build checks.
#[must_use]
pub fn process_rewards(steps: &[Step]) -> Vec<Reward> {
    let intents = intents_by_tool_use(steps);
    let mut out = Vec::new();
    // path → whether it has been read since its last write
    let mut fresh_reads: HashMap<String, bool> = HashMap::new();
    let mut last_verify_ok: Option<bool> = None;
    let mut last_build_ok: Option<bool> = None;
    for step in steps {
        match &step.action {
            StepAction::ToolResult {
                tool_use_id,
                tool,
                is_error,
                source,
                files,
                ..
            } => {
                let target = step_target(step);
                let at = step.at_ms;
                if source == "blocked" {
                    out.push(reward(
                        target,
                        RewardKind::Process,
                        "blocked_call",
                        -0.2,
                        PROCESS_SOURCE,
                        at,
                    ));
                    continue;
                }
                let intent = intents.get(tool_use_id).map_or_else(
                    || ToolIntent::classify(tool, &serde_json::Value::Null),
                    |(i, _)| *i,
                );
                match intent {
                    ToolIntent::Edit => {
                        if *is_error {
                            out.push(reward(
                                target,
                                RewardKind::Process,
                                "edit_failed",
                                -0.2,
                                PROCESS_SOURCE,
                                at,
                            ));
                        } else {
                            out.push(reward(
                                target,
                                RewardKind::Process,
                                "edit_applied",
                                0.2,
                                PROCESS_SOURCE,
                                at,
                            ));
                            for f in files {
                                fresh_reads.insert(f.path.clone(), false);
                            }
                        }
                    }
                    ToolIntent::Verify => {
                        let ok = !*is_error;
                        let (name, value) = match (last_verify_ok, ok) {
                            (Some(false), true) => ("verify_fixed", 0.5),
                            (_, true) => ("verify_passed", 0.3),
                            (Some(true), false) => ("verify_broke", -0.3),
                            (_, false) => ("verify_failed", -0.1),
                        };
                        out.push(reward(
                            target,
                            RewardKind::Process,
                            name,
                            value,
                            PROCESS_SOURCE,
                            at,
                        ));
                        last_verify_ok = Some(ok);
                    }
                    ToolIntent::Explore if tool == "read_file" && !*is_error => {
                        for f in files {
                            let seen = fresh_reads.insert(f.path.clone(), true);
                            if seen == Some(true) && source != "cache" {
                                out.push(reward(
                                    target,
                                    RewardKind::Process,
                                    "redundant_read",
                                    -0.1,
                                    PROCESS_SOURCE,
                                    at,
                                ));
                            }
                        }
                    }
                    _ if *is_error => {
                        out.push(reward(
                            target,
                            RewardKind::Process,
                            "tool_error",
                            -0.05,
                            PROCESS_SOURCE,
                            at,
                        ));
                    }
                    _ => {}
                }
            }
            StepAction::Harness {
                action: HarnessAction::Build { success, .. },
                name,
                ..
            } if name == "auto_build" => {
                let (label, value) = match (last_build_ok, *success) {
                    (Some(false), true) => ("build_fixed", 0.5),
                    (Some(true), false) => ("build_broke", -0.5),
                    _ => ("", 0.0),
                };
                if !label.is_empty() {
                    out.push(reward(
                        step_target(step),
                        RewardKind::Process,
                        label,
                        value,
                        PROCESS_SOURCE,
                        step.at_ms,
                    ));
                }
                last_build_ok = Some(*success);
            }
            _ => {}
        }
    }
    out
}

/// Tokens and list price of each model call, and wall time for the
/// run, kept as separate penalties so weighting is a training-time
/// choice. A call to a model with no known price has no `usd` reward.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn cost_rewards(steps: &[Step]) -> Vec<Reward> {
    let mut out = Vec::new();
    for step in steps {
        if let (StepAction::Model(_), Some(usage)) = (&step.action, step.usage) {
            let tokens = usage.input + usage.output + usage.cache_write;
            out.push(reward(
                step_target(step),
                RewardKind::Tokens,
                "tokens",
                -(tokens as f64) / 100_000.0,
                COST_SOURCE,
                step.at_ms,
            ));
            if let Some(usd) = step_cost_usd(&step.policy.id, usage) {
                out.push(reward(
                    step_target(step),
                    RewardKind::Cost,
                    "usd",
                    -usd,
                    COST_SOURCE,
                    step.at_ms,
                ));
            }
        }
    }
    if let (Some(first), Some(last)) = (steps.first(), steps.last()) {
        let minutes = last.at_ms.saturating_sub(first.at_ms) as f64 / 60_000.0;
        out.push(reward(
            RewardTarget::Run,
            RewardKind::Cost,
            "wall_minutes",
            -minutes / 60.0,
            COST_SOURCE,
            last.at_ms,
        ));
    }
    out
}

/// Whether the behavior a harness decision aimed for showed up within
/// [`EFFECT_WINDOW`] model calls.
#[must_use]
pub fn decision_effects(steps: &[Step]) -> Vec<Reward> {
    let mut out = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let Actor::Harness { source } = step.actor else {
            continue;
        };
        let StepAction::Harness { name, action, .. } = &step.action else {
            continue;
        };
        let window: Vec<&Step> = steps[i + 1..]
            .iter()
            .filter(|s| {
                matches!(s.action, StepAction::Model(_))
                    && s.actor == Actor::Model
                    && s.turn == step.turn
            })
            .take(EFFECT_WINDOW)
            .collect();
        let effective = match (source, name.as_str(), action) {
            (_, "implement_now" | "implement_now_block", _) => {
                Some(window.iter().any(|s| has_intent(s, ToolIntent::Edit)))
            }
            (_, "repeated_read", _) => window.first().map(|s| {
                !model_calls(s).all(|i| i == ToolIntent::Explore) || model_calls(s).next().is_none()
            }),
            (_, "failed_edit_guard", HarnessAction::Warn { .. }) => Some(edit_succeeded_after(
                steps,
                i,
                step.links.files.first().map(String::as_str),
            )),
            (_, "no_op_turn" | "unfinished_action" | "leaked_tool_markup", _) => {
                window.first().map(|s| {
                    let StepAction::Model(a) = &s.action else {
                        return false;
                    };
                    !a.tool_calls.is_empty() || !a.text.trim().is_empty()
                })
            }
            (DecisionSource::Budget, "iteration_budget_warning", _) => {
                Some(window.iter().any(|s| {
                    has_intent(s, ToolIntent::Edit) || has_intent(s, ToolIntent::Complete)
                }))
            }
            _ => None,
        };
        if let Some(effective) = effective {
            let mut r = reward(
                step_target(step),
                RewardKind::DecisionEffect,
                &format!("effect:{name}"),
                if effective { 1.0 } else { -1.0 },
                EFFECT_SOURCE,
                step.at_ms,
            );
            r.evidence = json!({ "window": EFFECT_WINDOW, "observed_model_calls": window.len() });
            out.push(r);
        }
    }
    out
}

fn model_calls(step: &Step) -> impl Iterator<Item = ToolIntent> + '_ {
    let calls = match &step.action {
        StepAction::Model(a) => a.tool_calls.as_slice(),
        _ => &[],
    };
    calls.iter().map(|c| c.intent)
}

fn has_intent(step: &Step, intent: ToolIntent) -> bool {
    model_calls(step).any(|i| i == intent)
}

fn edit_succeeded_after(steps: &[Step], from: usize, path: Option<&str>) -> bool {
    steps[from + 1..]
        .iter()
        .take_while(|s| s.turn == steps[from].turn)
        .any(|s| match &s.action {
            StepAction::ToolResult {
                is_error: false,
                files,
                ..
            } => files
                .iter()
                .any(|f| f.kind != "read" && path.is_none_or(|p| f.path == p)),
            _ => false,
        })
}

/// Spread a run-level outcome over the model steps whose edits touched
/// files that ended up changed, so file-level credit is visible per
/// step. Returns nothing when no edit touched a changed file.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn attribute_outcome(steps: &[Step], outcome: f64, at_ms: u64) -> Vec<Reward> {
    let changed: std::collections::HashSet<&str> = steps
        .iter()
        .filter(|s| matches!(s.action, StepAction::TurnEnd { .. }))
        .flat_map(|s| s.links.files.iter().map(String::as_str))
        .collect();
    let mut credited: Vec<u64> = steps
        .iter()
        .filter_map(|s| match &s.action {
            StepAction::ToolResult {
                is_error: false,
                files,
                ..
            } if files
                .iter()
                .any(|f| f.kind != "read" && changed.contains(f.path.as_str())) =>
            {
                s.parent
            }
            _ => None,
        })
        .collect();
    credited.sort_unstable();
    credited.dedup();
    if credited.is_empty() {
        return Vec::new();
    }
    let share = outcome / credited.len() as f64;
    credited
        .into_iter()
        .map(|seq| {
            reward(
                RewardTarget::Step { seq },
                RewardKind::Outcome,
                "outcome_credit",
                share,
                "credit/v1",
                at_ms,
            )
        })
        .collect()
}

/// Discounted return per step: the sum of a step's own rewards plus
/// `gamma`-discounted later rewards. Run-level rewards count at the
/// final step. Returns `(seq, reward, return)` in step order.
#[must_use]
pub fn returns(steps: &[Step], rewards: &[Reward], gamma: f64) -> Vec<(u64, f64, f64)> {
    let mut per_step: HashMap<u64, f64> = HashMap::new();
    let last = steps.last().map_or(0, |s| s.seq);
    for r in rewards {
        let seq = match r.target {
            RewardTarget::Step { seq } => seq,
            RewardTarget::Run => last,
        };
        *per_step.entry(seq).or_default() += r.value;
    }
    let mut out: Vec<(u64, f64, f64)> = steps
        .iter()
        .map(|s| {
            (
                s.seq,
                per_step.get(&s.seq).copied().unwrap_or_default(),
                0.0,
            )
        })
        .collect();
    let mut acc = 0.0;
    for row in out.iter_mut().rev() {
        acc = row.1 + gamma * acc;
        row.2 = acc;
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use aura_core_types::trace::{
        FileTouch, HarnessAction, ModelAction, Observation, PolicyRef, StepLinks, TokenUsage,
        ToolCallAction,
    };
    use serde_json::json;

    pub(crate) fn base(seq: u64, actor: Actor, action: StepAction) -> Step {
        Step {
            run_id: "r".into(),
            seq,
            parent: None,
            turn: 1,
            iteration: None,
            at_ms: seq * 1000,
            actor,
            observation: Observation::None,
            action,
            policy: PolicyRef::default(),
            links: StepLinks::default(),
            usage: None,
            duration_ms: None,
        }
    }

    pub(crate) fn model(seq: u64, calls: &[(&str, &str, serde_json::Value)]) -> Step {
        let mut s = base(
            seq,
            Actor::Model,
            StepAction::Model(ModelAction {
                tool_calls: calls
                    .iter()
                    .map(|(id, n, i)| ToolCallAction {
                        tool_use_id: (*id).into(),
                        name: (*n).into(),
                        input: i.clone(),
                        intent: ToolIntent::classify(n, i),
                    })
                    .collect(),
                ..ModelAction::default()
            }),
        );
        s.usage = Some(TokenUsage {
            input: 1000,
            output: 100,
            cache_write: 0,
            cache_read: 0,
        });
        s
    }

    pub(crate) fn result(
        seq: u64,
        parent: u64,
        id: &str,
        tool: &str,
        path: &str,
        kind: &str,
        err: bool,
    ) -> Step {
        let mut s = base(
            seq,
            Actor::Environment,
            StepAction::ToolResult {
                tool_use_id: id.into(),
                tool: tool.into(),
                is_error: err,
                source: "executor".into(),
                content_chars: 1,
                excerpt: String::new(),
                content: None,
                files: vec![FileTouch {
                    path: path.into(),
                    kind: kind.into(),
                    added: 1,
                    removed: 0,
                }],
            },
        );
        s.parent = Some(parent);
        s
    }

    pub(crate) fn nudge(seq: u64, name: &str) -> Step {
        base(
            seq,
            Actor::Harness {
                source: DecisionSource::Steering,
            },
            StepAction::Harness {
                name: name.into(),
                action: HarnessAction::Nudge { kind: name.into() },
                alternatives: vec![],
            },
        )
    }

    #[test]
    fn redundant_read_and_edit_rewards() {
        let steps = vec![
            model(1, &[("a", "read_file", json!({"path": "x"}))]),
            result(2, 1, "a", "read_file", "x", "read", false),
            model(3, &[("b", "read_file", json!({"path": "x"}))]),
            result(4, 3, "b", "read_file", "x", "read", false),
            model(5, &[("c", "edit_file", json!({"path": "x"}))]),
            result(6, 5, "c", "edit_file", "x", "modify", false),
            model(7, &[("d", "read_file", json!({"path": "x"}))]),
            result(8, 7, "d", "read_file", "x", "read", false),
        ];
        let names: Vec<String> = process_rewards(&steps)
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(names, vec!["redundant_read", "edit_applied"]);
    }

    #[test]
    fn implement_now_effect_is_positive_when_an_edit_follows() {
        let steps = vec![
            nudge(1, "implement_now"),
            model(2, &[("a", "read_file", json!({"path": "x"}))]),
            model(3, &[("b", "edit_file", json!({"path": "x"}))]),
        ];
        let effects = decision_effects(&steps);
        assert_eq!(effects.len(), 1);
        assert!((effects[0].value - 1.0).abs() < f64::EPSILON);
        let ignored = decision_effects(&[nudge(1, "implement_now"), model(2, &[])]);
        assert!((ignored[0].value + 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cost_is_priced_per_call_and_tokens_counted_apart() {
        let mut opus = model(1, &[]);
        opus.policy.id = "claude-opus-5-5".into();
        let mut local = model(2, &[]);
        local.policy.id = "local/qwen3:4b".into();
        let usd = step_cost_usd("claude-opus-5-5", opus.usage.unwrap()).unwrap();
        let rewards = cost_rewards(&[opus, local]);
        let values = |kind: RewardKind, name: &str| -> Vec<f64> {
            rewards
                .iter()
                .filter(|r| r.kind == kind && r.name == name)
                .map(|r| r.value)
                .collect()
        };
        assert_eq!(values(RewardKind::Tokens, "tokens"), vec![-0.011, -0.011]);
        assert_eq!(
            values(RewardKind::Cost, "usd"),
            vec![-usd],
            "a model with no known price has no usd reward"
        );
        assert_eq!(values(RewardKind::Cost, "wall_minutes").len(), 1);
    }

    #[test]
    fn returns_discount_backwards() {
        let steps = vec![model(1, &[]), model(2, &[])];
        let rewards = vec![reward(
            RewardTarget::Run,
            RewardKind::Outcome,
            "pass",
            1.0,
            "t",
            0,
        )];
        let r = returns(&steps, &rewards, 0.5);
        assert!((r[1].2 - 1.0).abs() < 1e-9);
        assert!((r[0].2 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn outcome_credit_goes_to_editing_model_steps() {
        let mut end = base(
            9,
            Actor::Environment,
            StepAction::TurnEnd {
                stop_reason: "end_turn".into(),
                iterations: 1,
                input_tokens: 0,
                output_tokens: 0,
                files_changed: 1,
                error: None,
            },
        );
        end.links.files = vec!["x".into()];
        let steps = vec![
            model(1, &[("c", "edit_file", json!({"path": "x"}))]),
            result(2, 1, "c", "edit_file", "x", "modify", false),
            end,
        ];
        let credit = attribute_outcome(&steps, 1.0, 0);
        assert_eq!(credit.len(), 1);
        assert_eq!(credit[0].target, RewardTarget::Step { seq: 1 });
    }
}
