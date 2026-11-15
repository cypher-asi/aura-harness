//! Per-run scorecard.
//!
//! One flat summary of how efficiently a run used tokens and time, how
//! much work it wasted, what the harness did, and how it ended. The
//! same struct is stored as `summary.json`, printed by `aura runs show`
//! and compared by `aura runs diff` and `aura eval compare`.

use std::collections::{BTreeMap, HashMap};

use aura_core_types::trace::{
    Actor, HarnessAction, Reward, RewardKind, RewardTarget, Step, StepAction, ToolIntent,
};
use serde::{Deserialize, Serialize};

use crate::bundle::RunMeta;

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
