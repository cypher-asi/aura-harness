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
    Actor, DecisionSource, FileTouch, HarnessAction, Reward, RewardKind, RewardTarget, Step,
    StepAction, ToolIntent,
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
    let mut labeler = ProcessLabeler {
        intents: intents_by_tool_use(steps),
        ..ProcessLabeler::default()
    };
    for step in steps {
        match &step.action {
            StepAction::ToolResult {
                tool_use_id,
                tool,
                is_error,
                source,
                files,
                ..
            } => labeler.tool_result(
                step,
                &ToolResultView {
                    tool_use_id,
                    tool,
                    is_error: *is_error,
                    source,
                    files,
                },
            ),
            StepAction::Harness {
                action: HarnessAction::Build { success, .. },
                name,
                ..
            } if name == "auto_build" => labeler.auto_build(step, *success),
            _ => {}
        }
    }
    labeler.out
}

/// The fields of a `StepAction::ToolResult` that process rewards read.
struct ToolResultView<'a> {
    tool_use_id: &'a str,
    tool: &'a str,
    is_error: bool,
    source: &'a str,
    files: &'a [FileTouch],
}

/// Running state of [`process_rewards`] across the step log.
#[derive(Default)]
struct ProcessLabeler {
    intents: HashMap<String, (ToolIntent, serde_json::Value)>,
    out: Vec<Reward>,
    /// path → whether it has been read since its last write
    fresh_reads: HashMap<String, bool>,
    last_verify_ok: Option<bool>,
    last_build_ok: Option<bool>,
}

impl ProcessLabeler {
    fn push(&mut self, target: RewardTarget, name: &str, value: f64, at: u64) {
        self.out.push(reward(
            target,
            RewardKind::Process,
            name,
            value,
            PROCESS_SOURCE,
            at,
        ));
    }

    fn tool_result(&mut self, step: &Step, result: &ToolResultView<'_>) {
        let target = step_target(step);
        let at = step.at_ms;
        if result.source == "blocked" {
            self.push(target, "blocked_call", -0.2, at);
            return;
        }
        let intent = self.intents.get(result.tool_use_id).map_or_else(
            || ToolIntent::classify(result.tool, &serde_json::Value::Null),
            |(i, _)| *i,
        );
        match intent {
            ToolIntent::Edit => self.edit(target, at, result),
            ToolIntent::Verify => self.verify(target, at, !result.is_error),
            ToolIntent::Explore if result.tool == "read_file" && !result.is_error => {
                self.read(target, at, result);
            }
            _ if result.is_error => self.push(target, "tool_error", -0.05, at),
            _ => {}
        }
    }

    fn edit(&mut self, target: RewardTarget, at: u64, result: &ToolResultView<'_>) {
        if result.is_error {
            self.push(target, "edit_failed", -0.2, at);
        } else {
            self.push(target, "edit_applied", 0.2, at);
            for f in result.files {
                self.fresh_reads.insert(f.path.clone(), false);
            }
        }
    }

    fn verify(&mut self, target: RewardTarget, at: u64, ok: bool) {
        let (name, value) = match (self.last_verify_ok, ok) {
            (Some(false), true) => ("verify_fixed", 0.5),
            (_, true) => ("verify_passed", 0.3),
            (Some(true), false) => ("verify_broke", -0.3),
            (_, false) => ("verify_failed", -0.1),
        };
        self.push(target, name, value, at);
        self.last_verify_ok = Some(ok);
    }

    fn read(&mut self, target: RewardTarget, at: u64, result: &ToolResultView<'_>) {
        for f in result.files {
            let seen = self.fresh_reads.insert(f.path.clone(), true);
            if seen == Some(true) && result.source != "cache" {
                self.push(target, "redundant_read", -0.1, at);
            }
        }
    }

    fn auto_build(&mut self, step: &Step, success: bool) {
        let (label, value) = match (self.last_build_ok, success) {
            (Some(false), true) => ("build_fixed", 0.5),
            (Some(true), false) => ("build_broke", -0.5),
            _ => ("", 0.0),
        };
        if !label.is_empty() {
            self.push(step_target(step), label, value, step.at_ms);
        }
        self.last_build_ok = Some(success);
    }
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
        let window = effect_window(&steps[i + 1..], step);
        let effective = decision_effective(steps, i, (source, name.as_str(), action), &window);
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

/// The first [`EFFECT_WINDOW`] model calls in `decision`'s turn among `later`.
fn effect_window<'a>(later: &'a [Step], decision: &Step) -> Vec<&'a Step> {
    later
        .iter()
        .filter(|s| {
            matches!(s.action, StepAction::Model(_))
                && s.actor == Actor::Model
                && s.turn == decision.turn
        })
        .take(EFFECT_WINDOW)
        .collect()
}

/// Whether the decision at `steps[i]` had its intended effect, or `None`
/// when the decision has no effect check.
fn decision_effective(
    steps: &[Step],
    i: usize,
    decision: (DecisionSource, &str, &HarnessAction),
    window: &[&Step],
) -> Option<bool> {
    match decision {
        (_, "implement_now" | "implement_now_block", _) => {
            Some(window.iter().any(|s| has_intent(s, ToolIntent::Edit)))
        }
        (_, "repeated_read", _) => window.first().map(|s| {
            !model_calls(s).all(|i| i == ToolIntent::Explore) || model_calls(s).next().is_none()
        }),
        (_, "failed_edit_guard", HarnessAction::Warn { .. }) => Some(edit_succeeded_after(
            steps,
            i,
            steps[i].links.files.first().map(String::as_str),
        )),
        (_, "no_op_turn" | "unfinished_action" | "leaked_tool_markup", _) => {
            window.first().map(|s| {
                let StepAction::Model(a) = &s.action else {
                    return false;
                };
                !a.tool_calls.is_empty() || !a.text.trim().is_empty()
            })
        }
        (DecisionSource::Budget, "iteration_budget_warning", _) => Some(
            window
                .iter()
                .any(|s| has_intent(s, ToolIntent::Edit) || has_intent(s, ToolIntent::Complete)),
        ),
        _ => None,
    }
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
pub(crate) mod tests;
