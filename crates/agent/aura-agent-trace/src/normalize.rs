//! One run, loaded and normalized.
//!
//! [`RunTrace`] joins a bundle's metadata, steps, stored rewards and
//! freshly derived rewards. Derived rewards are recomputed on every
//! load so labeler improvements apply to old runs too.

use aura_core_types::trace::{Reward, RewardKind, RewardTarget, Step};

use crate::bundle::{RunBundle, RunMeta};
use crate::graph::RunGraph;
use crate::records::RecordRow;
use crate::rewards;
use crate::scorecard::RunScorecard;
use crate::TraceError;

/// Discount used for per-step returns in exports.
pub const DEFAULT_GAMMA: f64 = 0.95;

/// A loaded run.
#[derive(Debug, Clone)]
pub struct RunTrace {
    /// `meta.json`.
    pub meta: RunMeta,
    /// Every step in order.
    pub steps: Vec<Step>,
    /// Rewards from `rewards.jsonl`.
    pub stored_rewards: Vec<Reward>,
    /// Rewards computed from the steps, including outcome credit.
    pub derived_rewards: Vec<Reward>,
    /// Kernel record rows from `record.jsonl`.
    pub records: Vec<RecordRow>,
}

impl RunTrace {
    /// Load and normalize `bundle`.
    ///
    /// # Errors
    ///
    /// Returns an error when `meta.json` is missing or malformed.
    pub fn load(bundle: &RunBundle) -> Result<Self, TraceError> {
        let meta = bundle.read_meta()?;
        let mut steps = bundle.read_steps()?;
        let records = bundle.read_records()?;
        crate::records::join(&mut steps, &records);
        let stored = bundle.read_rewards()?;
        let mut trace = Self::from_parts(meta, steps, stored);
        trace.records = records;
        Ok(trace)
    }

    /// Normalize already-loaded parts.
    #[must_use]
    pub fn from_parts(meta: RunMeta, mut steps: Vec<Step>, stored_rewards: Vec<Reward>) -> Self {
        steps.sort_by_key(|s| s.seq);
        steps.dedup_by_key(|s| s.seq);
        let mut derived = rewards::derive(&steps);
        if let Some(outcome) = run_outcome(&stored_rewards) {
            let at = steps.last().map_or(0, |s| s.at_ms);
            derived.extend(rewards::attribute_outcome(&steps, outcome, at));
        }
        Self {
            meta,
            steps,
            stored_rewards,
            derived_rewards: derived,
            records: Vec::new(),
        }
    }

    /// Stored and derived rewards together.
    #[must_use]
    pub fn rewards(&self) -> Vec<Reward> {
        let mut all = self.stored_rewards.clone();
        all.extend(self.derived_rewards.iter().cloned());
        all
    }

    /// The run's outcome, when verified or labeled.
    #[must_use]
    pub fn outcome(&self) -> Option<f64> {
        run_outcome(&self.stored_rewards)
    }

    /// Scorecard over every reward.
    #[must_use]
    pub fn scorecard(&self) -> RunScorecard {
        let mut card = RunScorecard::compute(&self.meta, &self.steps, &self.rewards());
        card.apply_tool_timings(&crate::health::tool_call_timings(&self.records));
        card
    }

    /// Linked graph of the run.
    #[must_use]
    pub fn graph(&self) -> RunGraph {
        let mut graph = RunGraph::new(&self.meta);
        for step in &self.steps {
            graph.apply(step);
        }
        for row in &self.records {
            graph.apply_record(row);
        }
        for reward in self.rewards() {
            graph.apply_reward(&reward);
        }
        graph
    }

    /// `(seq, reward, discounted return)` per step.
    #[must_use]
    pub fn returns(&self, gamma: f64) -> Vec<(u64, f64, f64)> {
        rewards::returns(&self.steps, &self.rewards(), gamma)
    }

    /// Step by sequence number.
    #[must_use]
    pub fn step(&self, seq: u64) -> Option<&Step> {
        self.steps.iter().find(|s| s.seq == seq)
    }
}

/// Latest run-level outcome or explicit label.
fn run_outcome(stored: &[Reward]) -> Option<f64> {
    stored
        .iter()
        .filter(|r| {
            r.target == RewardTarget::Run
                && matches!(r.kind, RewardKind::Outcome | RewardKind::HumanExplicit)
        })
        .max_by_key(|r| r.at_ms)
        .map(|r| r.value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::RunSource;
    use crate::rewards::tests::{base, model, result};
    use aura_core_types::trace::{Actor, StepAction};
    use serde_json::json;

    #[test]
    fn outcome_is_credited_to_editing_steps() {
        let mut end = base(
            3,
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
        let outcome = Reward {
            target: RewardTarget::Run,
            kind: RewardKind::Outcome,
            name: "verify".into(),
            value: 1.0,
            source: "verify".into(),
            at_ms: 5,
            evidence: serde_json::Value::Null,
        };
        let trace = RunTrace::from_parts(RunMeta::new("m", RunSource::Test), steps, vec![outcome]);
        assert_eq!(trace.outcome(), Some(1.0));
        assert!(trace
            .derived_rewards
            .iter()
            .any(|r| r.name == "outcome_credit" && r.target == RewardTarget::Step { seq: 1 }));
        assert_eq!(trace.scorecard().outcome, Some(1.0));
    }
}
