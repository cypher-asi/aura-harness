//! Per-run scorecard.
//!
//! One flat summary of how efficiently a run used tokens and time, how
//! much work it wasted, what the harness did, and how it ended. The
//! same struct is stored as `summary.json`, printed by `aura runs show`
//! and compared by `aura runs diff` and `aura eval compare`.

use std::collections::{BTreeMap, HashMap};

use aura_core_types::trace::{
    Actor, HarnessAction, Reward, RewardKind, RewardTarget, Step, StepAction, TokenUsage,
    ToolIntent,
};
use serde::{Deserialize, Serialize};

use crate::bundle::RunMeta;

/// Name of the stored [`RewardKind::Cost`] reward that carries the list
/// price of a child agent's model calls (`value` is minus the USD), so a
/// run's scorecard counts work it delegated.
pub const CHILD_COST_REWARD: &str = "child_cost_usd";

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
    /// List price of model calls made by child agents the run delegated
    /// to, in USD.
    pub child_cost_usd: f64,
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
}

impl RunScorecard {
    /// Total prompt and output tokens, including cache traffic.
    #[must_use]
    pub const fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }

    /// What the run cost at list prices, including delegated work.
    #[must_use]
    pub fn total_cost_usd(&self) -> f64 {
        self.cost_usd + self.child_cost_usd
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
        let mut card = Self {
            run_id: meta.run_id.clone(),
            model: meta.model.clone(),
            ..Self::default()
        };
        let start = steps.first().map_or(0, |s| s.at_ms);
        card.wall_ms = steps.last().map_or(0, |s| s.at_ms.saturating_sub(start));
        let mut changed: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut tokens_so_far = 0u64;
        for step in steps {
            match &step.action {
                StepAction::UserPrompt { .. } => card.turns += 1,
                StepAction::Model(_) => {
                    card.model_calls += 1;
                    card.model_ms += step.duration_ms.unwrap_or_default();
                    if let Some(u) = step.usage {
                        card.input_tokens += u.input;
                        card.output_tokens += u.output;
                        card.cache_read_tokens += u.cache_read;
                        card.cache_write_tokens += u.cache_write;
                        tokens_so_far += u.input + u.output + u.cache_read + u.cache_write;
                        match step_cost_usd(&step.policy.id, u) {
                            Some(cost) => card.cost_usd += cost,
                            None => card.unpriced_calls += 1,
                        }
                    }
                }
                StepAction::ToolResult {
                    tool,
                    is_error,
                    source,
                    files,
                    ..
                } => {
                    card.tool_calls += 1;
                    if *is_error {
                        card.tool_errors += 1;
                    }
                    match source.as_str() {
                        "blocked" => card.blocked_calls += 1,
                        "cache" => card.cache_hits += 1,
                        _ => {}
                    }
                    let intent = ToolIntent::classify(tool, &serde_json::Value::Null);
                    if intent == ToolIntent::Edit && *is_error {
                        card.failed_edits += 1;
                    }
                    if !is_error {
                        for f in files {
                            if f.kind == "read" {
                                card.reads += 1;
                            } else {
                                card.writes += 1;
                                changed.insert(f.path.as_str());
                                if card.ms_to_first_edit.is_none() {
                                    card.ms_to_first_edit = Some(step.at_ms.saturating_sub(start));
                                    card.tokens_to_first_edit = Some(tokens_so_far);
                                }
                            }
                        }
                    }
                }
                StepAction::Harness { name, action, .. } => {
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
                StepAction::TurnEnd { stop_reason, .. } => {
                    card.stop_reasons.push(stop_reason.clone());
                    for f in &step.links.files {
                        changed.insert(f.as_str());
                    }
                }
            }
        }
        card.files_changed = changed.len();

        let steps_by_seq: HashMap<u64, &Step> = steps.iter().map(|s| (s.seq, s)).collect();
        for r in rewards {
            if r.kind == RewardKind::Cost && r.name == CHILD_COST_REWARD {
                card.child_cost_usd -= r.value;
                continue;
            }
            *card
                .reward_totals
                .entry(r.kind.as_str().to_string())
                .or_default() += r.value;
            match (r.kind, r.target) {
                (RewardKind::Process, _) if r.name == "redundant_read" => card.redundant_reads += 1,
                (RewardKind::DecisionEffect, RewardTarget::Step { seq }) => {
                    let name = steps_by_seq
                        .get(&seq)
                        .and_then(|s| match &s.action {
                            StepAction::Harness { name, .. } => Some(name.clone()),
                            _ => None,
                        })
                        .unwrap_or_else(|| r.name.trim_start_matches("effect:").to_string());
                    let stat = card.effects.entry(name).or_default();
                    stat.measured += 1;
                    if r.value > 0.0 {
                        stat.effective += 1;
                    }
                }
                (RewardKind::Outcome | RewardKind::HumanExplicit, RewardTarget::Run) => {
                    card.outcome = Some(r.value);
                }
                _ => {}
            }
        }
        card
    }
}

/// List price of one model call, `None` when `model` has no known price.
#[must_use]
pub fn step_cost_usd(model: &str, usage: TokenUsage) -> Option<f64> {
    aura_model_reasoner::model_catalog::estimate_cost_usd(
        model,
        &aura_model_reasoner::Usage {
            input_tokens: usage.input,
            output_tokens: usage.output,
            cache_creation_input_tokens: Some(usage.cache_write),
            cache_read_input_tokens: Some(usage.cache_read),
        },
    )
}

/// The reward that adds a child agent's model spend to its parent run.
#[must_use]
pub fn child_cost_reward(cost_usd: f64, source: &str, evidence: serde_json::Value) -> Reward {
    Reward {
        target: RewardTarget::Run,
        kind: RewardKind::Cost,
        name: CHILD_COST_REWARD.into(),
        value: -cost_usd,
        source: source.into(),
        at_ms: crate::bundle::now_ms(),
        evidence,
    }
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
        ("cost_usd", card.total_cost_usd(), false),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::RunSource;
    use crate::rewards::tests::{model, nudge, result};
    use serde_json::json;

    #[test]
    fn counts_waste_and_first_edit() {
        let steps = vec![
            model(1, &[("a", "read_file", json!({"path": "x"}))]),
            result(2, 1, "a", "read_file", "x", "read", false),
            nudge(3, "implement_now"),
            model(4, &[("b", "edit_file", json!({"path": "x"}))]),
            result(5, 4, "b", "edit_file", "x", "modify", true),
            model(6, &[("c", "edit_file", json!({"path": "x"}))]),
            result(7, 6, "c", "edit_file", "x", "modify", false),
        ];
        let rewards = crate::rewards::derive(&steps);
        let meta = RunMeta::new("m", RunSource::Test);
        let card = RunScorecard::compute(&meta, &steps, &rewards);
        assert_eq!(card.model_calls, 3);
        assert_eq!(card.failed_edits, 1);
        assert_eq!(card.writes, 1);
        assert_eq!(card.ms_to_first_edit, Some(6000));
        assert_eq!(card.tokens_to_first_edit, Some(3300));
        assert_eq!(card.decisions.get("steering/implement_now"), Some(&1));
        assert_eq!(card.effects["implement_now"].rate(), Some(1.0));
    }

    #[test]
    fn prices_each_call_by_its_model_and_adds_child_cost() {
        let mut opus = model(1, &[]);
        opus.policy.id = "claude-opus-5-5".into();
        let mut haiku = model(2, &[]);
        haiku.policy.id = "claude-haiku-5-5".into();
        let mut local = model(3, &[]);
        local.policy.id = "local/qwen3:4b".into();
        let steps = vec![opus, haiku, local];
        let rewards = vec![child_cost_reward(0.25, "gate", json!({}))];
        let card = RunScorecard::compute(&RunMeta::new("m", RunSource::Test), &steps, &rewards);
        // 1000 in + 100 out: opus $0.006, haiku $0.00015.
        assert!((card.cost_usd - 0.006_15).abs() < 1e-9, "{}", card.cost_usd);
        assert_eq!(card.unpriced_calls, 1);
        assert!((card.child_cost_usd - 0.25).abs() < 1e-12);
        assert!((card.total_cost_usd() - 0.256_15).abs() < 1e-9);
        assert!(!card.reward_totals.contains_key("cost"));
        assert!(mean(&[card])["cost_usd"] > 0.25);
    }

    #[test]
    fn diff_flags_regressions_past_tolerance() {
        let base = RunScorecard {
            model_calls: 10,
            outcome: Some(1.0),
            ..RunScorecard::default()
        };
        let cand = RunScorecard {
            model_calls: 12,
            outcome: Some(1.0),
            ..RunScorecard::default()
        };
        let d = diff(&base, &cand, 0.1);
        assert_eq!(d.len(), 1);
        assert!(d[0].regression);
        assert!(!diff(&base, &cand, 0.5)[0].regression);
    }
}
