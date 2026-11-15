//! The routing policy: which tier starts a block.
//!
//! Version 0 is a fixed rule table keyed by block kind, risk and whether
//! the result is checked. Fitted versions add per-bucket statistics; a
//! tier cheaper than the rule's is allowed only once its pass rate is
//! *earned* — the lower confidence bound over at least `min_samples`
//! attempts clears `pass_target`. Among allowed tiers the policy picks
//! the lowest expected total cost, counting the escalation a failure
//! causes.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::block::{BlockKind, BlockSpec, Risk};
use crate::tier::{Tier, Tiers};

/// Version string of the built-in rule table.
pub const RULES_VERSION: &str = "v0-rules";

/// Attempts and passes of one tier in one bucket.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TierStats {
    /// Attempts counted.
    pub attempts: u32,
    /// Of those, attempts that passed.
    pub passes: u32,
    /// Summed list price of the attempts, USD.
    pub cost_usd: f64,
}

impl TierStats {
    /// Posterior mean pass rate under a uniform prior.
    #[must_use]
    pub fn pass_rate(&self) -> f64 {
        (f64::from(self.passes) + 1.0) / (f64::from(self.attempts) + 2.0)
    }

    /// Mean cost of an attempt, when any were counted.
    #[must_use]
    pub fn mean_cost(&self) -> Option<f64> {
        (self.attempts > 0).then(|| self.cost_usd / f64::from(self.attempts))
    }
}

/// Statistics of one bucket.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BucketStats {
    /// Per-tier statistics.
    #[serde(default)]
    pub tiers: BTreeMap<Tier, TierStats>,
}

/// Lower bound of the Wilson score interval for `passes` out of `n` at
/// normal quantile `z`. `0.0` when `n == 0`.
#[must_use]
pub fn wilson_lower(passes: u32, n: u32, z: f64) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let n = f64::from(n);
    let p = f64::from(passes) / n;
    let z2 = z * z;
    let center = p + z2 / (2.0 * n);
    let margin = z * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt();
    ((center - margin) / (1.0 + z2 / n)).max(0.0)
}

/// A versioned routing policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GatePolicy {
    /// Version name; fitted policies get a new one.
    pub version: String,
    /// Version this one was fitted from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Pass rate a cheaper tier must earn before it starts a bucket.
    pub pass_target: f64,
    /// Attempts needed before a pass rate counts as earned.
    pub min_samples: u32,
    /// Normal quantile of the confidence bound (1.64 is one-sided 95%).
    pub z: f64,
    /// Share of eligible blocks started one tier cheaper than the
    /// policy's choice, to learn whether that tier would do.
    pub explore_rate: f64,
    /// Worker runs per block before it goes back to the coordinator.
    pub max_attempts: u32,
    /// Roll back a failed attempt's changes before escalating.
    pub restart_on_escalate: bool,
    /// Let the coordinator edit files itself instead of only through
    /// `run_blocks`.
    pub coordinator_edits: bool,
    /// Learned statistics by bucket.
    pub buckets: BTreeMap<String, BucketStats>,
}

impl Default for GatePolicy {
    fn default() -> Self {
        Self {
            version: RULES_VERSION.into(),
            parent: None,
            pass_target: 0.9,
            min_samples: 8,
            z: 1.64,
            explore_rate: 0.0,
            max_attempts: 4,
            restart_on_escalate: true,
            coordinator_edits: false,
            buckets: BTreeMap::new(),
        }
    }
}

/// How one tier scored for a block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Tier.
    pub tier: Tier,
    /// Model it runs.
    pub model: String,
    /// Estimated pass probability.
    pub p_pass: f64,
    /// Confidence lower bound of the pass rate.
    pub lower: f64,
    /// Attempts behind the estimate.
    pub samples: u32,
    /// Expected cost including escalation after a failure, USD.
    pub expected_cost_usd: f64,
    /// Whether the policy may start the block here.
    pub allowed: bool,
}

/// The tier a block starts on, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    /// Chosen tier.
    pub tier: Tier,
    /// `rule`, `earned`, `cost`, `explore` or `min_tier`.
    pub reason: String,
    /// Probability of this choice under the policy.
    pub propensity: f64,
    /// Chosen by exploration.
    pub explored: bool,
    /// Every tier considered.
    pub candidates: Vec<Candidate>,
}

/// Prior pass rates for tiers without data, cheapest first.
const PRIOR_PASS: [f64; 3] = [0.75, 0.88, 0.95];

impl GatePolicy {
    /// Load a policy from JSON.
    ///
    /// # Errors
    /// Fails when the file can't be read or parsed.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading gate policy {}: {e}", path.display()))?;
        serde_json::from_str(&text)
            .map_err(|e| format!("parsing gate policy {}: {e}", path.display()))
    }

    /// Write the policy as pretty JSON, creating parent directories.
    ///
    /// # Errors
    /// Fails when the file can't be written.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Tier the fixed rules start `spec` on, ignoring statistics.
    #[must_use]
    pub fn rule_tier(spec: &BlockSpec) -> Tier {
        let base = match (spec.kind, spec.risk) {
            (BlockKind::Command, _) => Tier::Fast,
            (_, Risk::High) => Tier::Frontier,
            (BlockKind::Explore | BlockKind::Docs, _)
            | (BlockKind::Edit | BlockKind::Test, Risk::Low) => Tier::Fast,
            (BlockKind::Edit | BlockKind::Test | BlockKind::Refactor, _) => Tier::Standard,
        };
        // Without checks only the worker's word says it worked, so the
        // cheapest tier is not trusted with changes.
        let floor = if spec.kind.writes() && !spec.verifiable() {
            Tier::Standard
        } else {
            Tier::Fast
        };
        base.max(floor).max(spec.min_tier.unwrap_or(Tier::Fast))
    }

    /// Choose the starting tier for `spec`. `roll` is a uniform sample in
    /// `[0, 1)`; `can_explore` is false when exploration is out of budget
    /// or the block can't be undone.
    #[must_use]
    pub fn choose(&self, spec: &BlockSpec, tiers: &Tiers, roll: f64, can_explore: bool) -> Choice {
        let rule = Self::rule_tier(spec);
        let floor = spec.min_tier.unwrap_or(Tier::Fast);
        let bucket = self.buckets.get(&spec.bucket());
        let stats = |t: Tier| {
            bucket
                .and_then(|b| b.tiers.get(&t))
                .copied()
                .unwrap_or_default()
        };
        let p_pass = |t: Tier| {
            let s = stats(t);
            if s.attempts == 0 {
                PRIOR_PASS[t as usize]
            } else {
                s.pass_rate()
            }
        };
        let cost = |t: Tier| {
            stats(t)
                .mean_cost()
                .unwrap_or_else(|| prior_cost(tiers.model(t)))
        };
        // Expected spend until the block passes or reaches the frontier:
        // E(t) = c(t) + (1 - p(t)) * E(stronger(t)).
        let mut expected = [0.0f64; 3];
        for t in Tier::ALL.into_iter().rev() {
            let next = tiers.stronger(t).map_or(0.0, |n| expected[n as usize]);
            expected[t as usize] = cost(t) + (1.0 - p_pass(t)) * next;
        }
        let candidates: Vec<Candidate> = Tier::ALL
            .into_iter()
            .filter(|t| *t >= floor)
            .map(|t| {
                let s = stats(t);
                let lower = wilson_lower(s.passes, s.attempts, self.z);
                let earned = s.attempts >= self.min_samples && lower >= self.pass_target;
                Candidate {
                    tier: t,
                    model: tiers.model(t).to_string(),
                    p_pass: p_pass(t),
                    lower,
                    samples: s.attempts,
                    expected_cost_usd: expected[t as usize],
                    allowed: t >= rule || earned,
                }
            })
            .collect();
        let best = candidates
            .iter()
            .filter(|c| c.allowed)
            .min_by(|a, b| a.expected_cost_usd.total_cmp(&b.expected_cost_usd))
            .map_or(rule, |c| c.tier);
        let reason = if best < rule {
            "earned"
        } else if best > rule {
            "cost"
        } else if spec.min_tier.is_some_and(|m| m == rule && m > Tier::Fast) {
            "min_tier"
        } else {
            "rule"
        };
        let cheaper = tiers.cheaper(best).filter(|t| *t >= floor);
        let explorable =
            can_explore && self.explore_rate > 0.0 && spec.verifiable() && cheaper.is_some();
        match cheaper {
            Some(tier) if explorable && roll < self.explore_rate => Choice {
                tier,
                reason: "explore".into(),
                propensity: self.explore_rate,
                explored: true,
                candidates,
            },
            _ => Choice {
                tier: best,
                reason: reason.into(),
                propensity: if explorable {
                    1.0 - self.explore_rate
                } else {
                    1.0
                },
                explored: false,
                candidates,
            },
        }
    }
}

/// Prior cost of one block attempt on `model`: list price of a typical
/// worker run (mostly cached prompt reads), `1.0` for unpriced models.
#[must_use]
pub fn prior_cost(model: &str) -> f64 {
    aura_model_reasoner::model_catalog::estimate_cost_usd(
        model,
        &aura_model_reasoner::Usage {
            input_tokens: 30_000,
            output_tokens: 4_000,
            cache_creation_input_tokens: Some(10_000),
            cache_read_input_tokens: Some(150_000),
        },
    )
    .unwrap_or(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::tests::edit;

    fn tiers() -> Tiers {
        Tiers::for_frontier("claude-opus-5-5")
    }

    #[test]
    fn wilson_bound_is_conservative() {
        assert!(wilson_lower(0, 0, 1.64).abs() < f64::EPSILON);
        let small = wilson_lower(8, 8, 1.64);
        let large = wilson_lower(80, 80, 1.64);
        assert!(small < 0.9, "8/8 should not yet clear 0.9: {small}");
        assert!(large > 0.9, "80/80 should clear 0.9: {large}");
        assert!(wilson_lower(45, 50, 1.64) < 0.9);
    }

    #[test]
    fn rules_follow_kind_risk_and_checks() {
        let mut spec = edit("a");
        assert_eq!(GatePolicy::rule_tier(&spec), Tier::Standard);
        spec.risk = Risk::Low;
        assert_eq!(GatePolicy::rule_tier(&spec), Tier::Standard, "unchecked");
        spec.checks.push("cargo test".into());
        assert_eq!(GatePolicy::rule_tier(&spec), Tier::Fast);
        spec.risk = Risk::High;
        assert_eq!(GatePolicy::rule_tier(&spec), Tier::Frontier);
        spec.kind = BlockKind::Explore;
        spec.risk = Risk::Medium;
        assert_eq!(GatePolicy::rule_tier(&spec), Tier::Fast);
        spec.min_tier = Some(Tier::Standard);
        assert_eq!(GatePolicy::rule_tier(&spec), Tier::Standard);
    }

    #[test]
    fn without_data_the_rule_tier_wins() {
        let mut spec = edit("a");
        spec.checks.push("t".into());
        let choice = GatePolicy::default().choose(&spec, &tiers(), 0.5, true);
        assert_eq!(choice.tier, Tier::Standard);
        assert_eq!(choice.reason, "rule");
        assert!((choice.propensity - 1.0).abs() < f64::EPSILON);
        assert!(!choice.candidates[0].allowed);
    }

    #[test]
    fn a_cheaper_tier_must_earn_its_bucket() {
        let mut spec = edit("a");
        spec.checks.push("t".into());
        let mut policy = GatePolicy::default();
        let mut bucket = BucketStats::default();
        bucket.tiers.insert(
            Tier::Fast,
            TierStats {
                attempts: 10,
                passes: 10,
                cost_usd: 0.1,
            },
        );
        policy.buckets.insert(spec.bucket(), bucket.clone());
        // 10/10 has a lower bound under 0.9.
        assert_eq!(
            policy.choose(&spec, &tiers(), 0.5, false).tier,
            Tier::Standard
        );

        bucket.tiers.get_mut(&Tier::Fast).unwrap().attempts = 60;
        bucket.tiers.get_mut(&Tier::Fast).unwrap().passes = 59;
        policy.buckets.insert(spec.bucket(), bucket);
        let choice = policy.choose(&spec, &tiers(), 0.5, false);
        assert_eq!(choice.tier, Tier::Fast);
        assert_eq!(choice.reason, "earned");
    }

    #[test]
    fn a_failing_rule_tier_loses_on_expected_cost() {
        let mut spec = edit("a");
        spec.checks.push("t".into());
        let mut policy = GatePolicy::default();
        let mut bucket = BucketStats::default();
        bucket.tiers.insert(
            Tier::Standard,
            TierStats {
                attempts: 20,
                passes: 2,
                cost_usd: 20.0 * prior_cost("claude-sonnet-5-5"),
            },
        );
        policy.buckets.insert(spec.bucket(), bucket);
        let choice = policy.choose(&spec, &tiers(), 0.5, false);
        assert_eq!(choice.tier, Tier::Frontier);
        assert_eq!(choice.reason, "cost");
    }

    #[test]
    fn exploration_tries_one_tier_cheaper_and_logs_propensity() {
        let mut spec = edit("a");
        spec.checks.push("t".into());
        let policy = GatePolicy {
            explore_rate: 0.1,
            ..GatePolicy::default()
        };
        let explored = policy.choose(&spec, &tiers(), 0.05, true);
        assert_eq!(explored.tier, Tier::Fast);
        assert!(explored.explored);
        assert!((explored.propensity - 0.1).abs() < 1e-9);
        let exploited = policy.choose(&spec, &tiers(), 0.5, true);
        assert_eq!(exploited.tier, Tier::Standard);
        assert!((exploited.propensity - 0.9).abs() < 1e-9);
        // Unchecked blocks are never explored.
        spec.checks.clear();
        assert!(!policy.choose(&spec, &tiers(), 0.0, true).explored);
    }

    #[test]
    fn policy_round_trips_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        let mut policy = GatePolicy::default();
        policy
            .buckets
            .entry("edit/low/checked/small".into())
            .or_default()
            .tiers
            .insert(Tier::Fast, TierStats::default());
        policy.save(&path).unwrap();
        assert_eq!(GatePolicy::load(&path).unwrap(), policy);
    }
}
