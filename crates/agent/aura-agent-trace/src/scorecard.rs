//! Per-run scorecard.
//!
//! One flat summary of how efficiently a run used tokens and time, how
//! much work it wasted, what the harness did, and how it ended. The
//! same struct is stored as `summary.json`, printed by `aura runs show`
//! and compared by `aura runs diff` and `aura eval compare`.

use std::collections::{BTreeMap, HashMap};

use aura_core_types::trace::{
    Actor, FileTouch, HarnessAction, Reward, RewardKind, RewardTarget, Step, StepAction,
    TokenUsage, ToolIntent,
};
use serde::{Deserialize, Serialize};

use crate::bundle::RunMeta;
use crate::health::ToolCallTiming;

/// How often a harness decision fired and how often its intended
/// behavior followed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct EffectStat {
    /// Decisions with a measurable effect.
    pub measured: usize,
    /// Of those, how many were followed by the intended behavior.
    pub effective: usize,
}

impl EffectStat {
    /// `effective / measured`, or `None` when nothing was measured.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn rate(&self) -> Option<f64> {
        (self.measured > 0).then(|| self.effective as f64 / self.measured as f64)
    }
}

/// Summary metrics of one run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunScorecard {
    /// Run id.
    pub run_id: String,
    /// Model requested for the run.
    pub model: String,
    /// User turns.
    pub turns: u32,
    /// Model calls.
    pub model_calls: usize,
    /// Tool calls with a result.
    pub tool_calls: usize,
    /// Tool calls that returned an error.
    pub tool_errors: usize,
    /// Uncached prompt tokens.
    pub input_tokens: u64,
    /// Generated tokens.
    pub output_tokens: u64,
    /// Prompt tokens read from the provider cache.
    pub cache_read_tokens: u64,
    /// Prompt tokens written to the provider cache.
    pub cache_write_tokens: u64,
    /// List price of the run's own model calls, in USD.
    pub cost_usd: f64,
    /// Model calls to models without a known price (not in `cost_usd`).
    pub unpriced_calls: usize,
    /// Wall time from first to last step.
    pub wall_ms: u64,
    /// Time spent waiting on the model.
    pub model_ms: u64,
    /// Time from the first prompt to the first successful edit.
    pub ms_to_first_edit: Option<u64>,
    /// Prompt and output tokens spent before the first successful edit.
    pub tokens_to_first_edit: Option<u64>,
    /// Successful file reads.
    pub reads: usize,
    /// Successful file mutations.
    pub writes: usize,
    /// Reads of a file already read since its last write.
    pub redundant_reads: usize,
    /// Failed edit attempts.
    pub failed_edits: usize,
    /// Tool calls refused by the harness or policy.
    pub blocked_calls: usize,
    /// Tool calls served from the result cache.
    pub cache_hits: usize,
    /// Provider retries and stream resets.
    pub retries: usize,
    /// Harness decisions by `source/name`.
    pub decisions: BTreeMap<String, usize>,
    /// Compactions by tier.
    pub compactions: BTreeMap<String, usize>,
    /// Decision effectiveness by heuristic name.
    pub effects: BTreeMap<String, EffectStat>,
    /// Stop reason of each turn.
    pub stop_reasons: Vec<String>,
    /// Distinct files changed.
    pub files_changed: usize,
    /// Sum of rewards by kind.
    pub reward_totals: BTreeMap<String, f64>,
    /// Run-level outcome (verification result or explicit label).
    pub outcome: Option<f64>,
    /// Tool calls that waited for someone to approve them.
    pub approvals_asked: usize,
    /// Of those, how many were denied (including unanswered prompts that
    /// timed out).
    pub approvals_denied: usize,
    /// Total time tool calls waited for approval.
    pub approval_wait_ms: u64,
    /// Longest single approval wait.
    pub max_approval_wait_ms: u64,
    /// Tool calls proposed but not finished when the run was loaded:
    /// never decided, or allowed but never run.
    pub open_tool_calls: usize,
}

impl RunScorecard {
    /// Total prompt and output tokens, including cache traffic.
    #[must_use]
    pub const fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }

    /// Share of prompt tokens served from the provider cache.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn cache_ratio(&self) -> f64 {
        let prompt = self.input_tokens + self.cache_read_tokens + self.cache_write_tokens;
        if prompt == 0 {
            0.0
        } else {
            self.cache_read_tokens as f64 / prompt as f64
        }
    }

    /// Reads per successful write; `None` before any write.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn read_write_ratio(&self) -> Option<f64> {
        (self.writes > 0).then(|| self.reads as f64 / self.writes as f64)
    }

    /// Compute the scorecard. `rewards` should hold stored and derived
    /// rewards together.
    #[must_use]
    pub fn compute(meta: &RunMeta, steps: &[Step], rewards: &[Reward]) -> Self {
        let card = Self {
            run_id: meta.run_id.clone(),
            model: meta.model.clone(),
            ..Self::default()
        };
        let mut tally = StepTally::new(card, steps);
        for step in steps {
            tally.observe(step);
        }
        let mut card = tally.finish();
        card.apply_rewards(steps, rewards);
        card
    }

    fn apply_rewards(&mut self, steps: &[Step], rewards: &[Reward]) {
        let steps_by_seq: HashMap<u64, &Step> = steps.iter().map(|s| (s.seq, s)).collect();
        for r in rewards {
            *self
                .reward_totals
                .entry(r.kind.as_str().to_string())
                .or_default() += r.value;
            match (r.kind, r.target) {
                (RewardKind::Process, _) if r.name == "redundant_read" => self.redundant_reads += 1,
                (RewardKind::DecisionEffect, RewardTarget::Step { seq }) => {
                    let name = steps_by_seq
                        .get(&seq)
                        .and_then(|s| match &s.action {
                            StepAction::Harness { name, .. } => Some(name.clone()),
                            _ => None,
                        })
                        .unwrap_or_else(|| r.name.trim_start_matches("effect:").to_string());
                    let stat = self.effects.entry(name).or_default();
                    stat.measured += 1;
                    if r.value > 0.0 {
                        stat.effective += 1;
                    }
                }
                (RewardKind::Outcome | RewardKind::HumanExplicit, RewardTarget::Run) => {
                    self.outcome = Some(r.value);
                }
                _ => {}
            }
        }
    }

    /// Fill the approval and open-call counts from the kernel record log.
    pub fn apply_tool_timings(&mut self, timings: &[ToolCallTiming]) {
        for call in timings {
            if call.asked_for_approval() {
                let wait = call.decision_ms().unwrap_or_default();
                self.approvals_asked += 1;
                self.approval_wait_ms += wait;
                self.max_approval_wait_ms = self.max_approval_wait_ms.max(wait);
                if call.denied() {
                    self.approvals_denied += 1;
                }
            }
            if call.is_open() {
                self.open_tool_calls += 1;
            }
        }
    }
}

/// List price of one model call, `None` when `model` has no known price.
#[must_use]
pub fn step_cost_usd(model: &str, usage: TokenUsage) -> Option<f64> {
    aura_model::model_catalog::estimate_cost_usd(
        model,
        &aura_model::Usage {
            input_tokens: usage.input,
            output_tokens: usage.output,
            cache_creation_input_tokens: Some(usage.cache_write),
            cache_read_input_tokens: Some(usage.cache_read),
        },
    )
}

/// One metric that differs between two scorecards.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricDelta {
    /// Metric name.
    pub metric: String,
    /// Baseline value.
    pub baseline: f64,
    /// Candidate value.
    pub candidate: f64,
    /// Whether the change is a regression (worse for the candidate).
    pub regression: bool,
}

/// Headline numeric metrics with the direction that counts as better.
#[allow(clippy::cast_precision_loss)]
fn headline(card: &RunScorecard) -> Vec<(&'static str, f64, bool)> {
    vec![
        ("outcome", card.outcome.unwrap_or_default(), true),
        ("cost_usd", card.cost_usd, false),
        ("total_tokens", card.total_tokens() as f64, false),
        ("model_calls", card.model_calls as f64, false),
        ("wall_ms", card.wall_ms as f64, false),
        ("tool_errors", card.tool_errors as f64, false),
        ("redundant_reads", card.redundant_reads as f64, false),
        ("failed_edits", card.failed_edits as f64, false),
        ("blocked_calls", card.blocked_calls as f64, false),
        ("cache_ratio", card.cache_ratio(), true),
        (
            "tokens_to_first_edit",
            card.tokens_to_first_edit.unwrap_or_default() as f64,
            false,
        ),
    ]
}

/// Compare headline metrics. A change counts as a regression when the
/// candidate is worse by more than `tolerance` (relative, e.g. `0.1`).
#[must_use]
pub fn diff(baseline: &RunScorecard, candidate: &RunScorecard, tolerance: f64) -> Vec<MetricDelta> {
    headline(baseline)
        .into_iter()
        .zip(headline(candidate))
        .filter_map(|((metric, b, higher_better), (_, c, _))| {
            delta(metric, b, c, higher_better, tolerance)
        })
        .collect()
}

/// [`diff`] over per-arm means from [`mean`].
#[must_use]
pub fn diff_means(
    baseline: &BTreeMap<String, f64>,
    candidate: &BTreeMap<String, f64>,
    tolerance: f64,
) -> Vec<MetricDelta> {
    headline(&RunScorecard::default())
        .into_iter()
        .filter_map(|(metric, _, higher_better)| {
            let b = baseline.get(metric).copied().unwrap_or_default();
            let c = candidate.get(metric).copied().unwrap_or_default();
            delta(metric, b, c, higher_better, tolerance)
        })
        .collect()
}

fn delta(metric: &str, b: f64, c: f64, higher_better: bool, tolerance: f64) -> Option<MetricDelta> {
    if (b - c).abs() <= f64::EPSILON {
        return None;
    }
    let worse = if higher_better { c < b } else { c > b };
    let rel = if b.abs() > f64::EPSILON {
        (c - b).abs() / b.abs()
    } else {
        1.0
    };
    Some(MetricDelta {
        metric: metric.to_string(),
        baseline: b,
        candidate: c,
        regression: worse && rel > tolerance,
    })
}

/// Mean of each headline metric across scorecards, for eval suites.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn mean(cards: &[RunScorecard]) -> BTreeMap<String, f64> {
    let mut sums: BTreeMap<String, f64> = BTreeMap::new();
    for card in cards {
        for (name, value, _) in headline(card) {
            *sums.entry(name.to_string()).or_default() += value;
        }
    }
    let n = cards.len().max(1) as f64;
    sums.into_iter().map(|(k, v)| (k, v / n)).collect()
}

/// Running state while [`RunScorecard::compute`] walks the steps.
struct StepTally<'a> {
    card: RunScorecard,
    changed: std::collections::HashSet<&'a str>,
    tokens_so_far: u64,
    prompted: Option<u32>,
    start: u64,
}

impl<'a> StepTally<'a> {
    fn new(mut card: RunScorecard, steps: &[Step]) -> Self {
        let start = steps.first().map_or(0, |s| s.at_ms);
        card.wall_ms = steps.last().map_or(0, |s| s.at_ms.saturating_sub(start));
        Self {
            card,
            changed: std::collections::HashSet::new(),
            tokens_so_far: 0,
            prompted: None,
            start,
        }
    }

    fn finish(mut self) -> RunScorecard {
        self.card.files_changed = self.changed.len();
        self.card
    }

    fn observe(&mut self, step: &'a Step) {
        match &step.action {
            StepAction::UserPrompt { .. } => {
                if self.prompted.replace(step.turn) != Some(step.turn) {
                    self.card.turns += 1;
                }
            }
            StepAction::Model(_) => self.observe_model(step),
            StepAction::ToolResult {
                tool,
                is_error,
                source,
                files,
                ..
            } => self.observe_tool_result(step, tool, *is_error, source, files),
            StepAction::Harness { name, action, .. } => self.observe_harness(step, name, action),
            StepAction::TurnEnd { stop_reason, .. } => {
                self.card.stop_reasons.push(stop_reason.clone());
                for f in &step.links.files {
                    self.changed.insert(f.as_str());
                }
            }
        }
    }

    fn observe_model(&mut self, step: &Step) {
        let card = &mut self.card;
        card.model_calls += 1;
        card.model_ms += step.duration_ms.unwrap_or_default();
        if let Some(u) = step.usage {
            card.input_tokens += u.input;
            card.output_tokens += u.output;
            card.cache_read_tokens += u.cache_read;
            card.cache_write_tokens += u.cache_write;
            self.tokens_so_far += u.input + u.output + u.cache_read + u.cache_write;
            match step_cost_usd(&step.policy.id, u) {
                Some(cost) => card.cost_usd += cost,
                None => card.unpriced_calls += 1,
            }
        }
    }

    fn observe_tool_result(
        &mut self,
        step: &Step,
        tool: &str,
        is_error: bool,
        source: &str,
        files: &'a [FileTouch],
    ) {
        let card = &mut self.card;
        card.tool_calls += 1;
        if is_error {
            card.tool_errors += 1;
        }
        match source {
            "blocked" => card.blocked_calls += 1,
            "cache" => card.cache_hits += 1,
            _ => {}
        }
        let intent = ToolIntent::classify(tool, &serde_json::Value::Null);
        if intent == ToolIntent::Edit && is_error {
            card.failed_edits += 1;
        }
        if !is_error {
            for f in files {
                if f.kind == "read" {
                    card.reads += 1;
                } else {
                    card.writes += 1;
                    self.changed.insert(f.path.as_str());
                    if card.ms_to_first_edit.is_none() {
                        card.ms_to_first_edit = Some(step.at_ms.saturating_sub(self.start));
                        card.tokens_to_first_edit = Some(self.tokens_so_far);
                    }
                }
            }
        }
    }

    fn observe_harness(&mut self, step: &Step, name: &str, action: &HarnessAction) {
        let card = &mut self.card;
        let source = match step.actor {
            Actor::Harness { source } => source.as_str(),
            _ => "harness",
        };
        *card
            .decisions
            .entry(format!("{source}/{name}"))
            .or_default() += 1;
        match action {
            HarnessAction::Compact { tier, .. } => {
                *card.compactions.entry(tier.clone()).or_default() += 1;
            }
            HarnessAction::Retry { .. } => card.retries += 1,
            HarnessAction::Block { .. } if step.parent.is_none() => {
                card.blocked_calls += 1;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
