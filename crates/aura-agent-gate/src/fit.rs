//! Fitting a policy from the ledger, and reporting on it.
//!
//! Only first attempts count: they are the outcomes of the choice the
//! policy makes ("start this bucket on this tier"). Retries and
//! escalations see harder, already-failed instances and would bias the
//! estimate. An attempt that passed its checks inside a recorded run
//! whose final verification failed counts as a failure, so a tier can't
//! earn a bucket on checks that were too weak.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use crate::ledger::LedgerEntry;
use crate::policy::{wilson_lower, BucketStats, GatePolicy};
use crate::tier::Tier;

/// Statistics by bucket from `entries`. `run_outcomes` maps run ids to
/// whether the run's final verification passed.
#[must_use]
pub fn bucket_stats(
    entries: &[LedgerEntry],
    run_outcomes: &HashMap<String, bool>,
) -> BTreeMap<String, BucketStats> {
    let mut buckets: BTreeMap<String, BucketStats> = BTreeMap::new();
    for e in entries.iter().filter(|e| e.attempt == 1) {
        let run_ok = e
            .run_id
            .as_ref()
            .and_then(|r| run_outcomes.get(r))
            .copied()
            .unwrap_or(true);
        let stats = buckets
            .entry(e.bucket.clone())
            .or_default()
            .tiers
            .entry(e.tier)
            .or_default();
        stats.attempts += 1;
        if e.passed && run_ok {
            stats.passes += 1;
        }
        stats.cost_usd += e.cost_usd;
    }
    buckets
}

/// A new policy version: `base`'s settings with statistics refitted from
/// `entries`.
#[must_use]
pub fn fit(
    base: &GatePolicy,
    entries: &[LedgerEntry],
    run_outcomes: &HashMap<String, bool>,
    version: &str,
) -> GatePolicy {
    GatePolicy {
        version: version.to_string(),
        parent: Some(base.version.clone()),
        buckets: bucket_stats(entries, run_outcomes),
        ..base.clone()
    }
}

/// Tiers that have earned each bucket under `policy`'s thresholds.
#[must_use]
pub fn earned_tiers(policy: &GatePolicy) -> BTreeMap<String, Vec<Tier>> {
    policy
        .buckets
        .iter()
        .map(|(bucket, stats)| {
            let earned = stats
                .tiers
                .iter()
                .filter(|(_, s)| {
                    s.attempts >= policy.min_samples
                        && wilson_lower(s.passes, s.attempts, policy.z) >= policy.pass_target
                })
                .map(|(t, _)| *t)
                .collect();
            (bucket.clone(), earned)
        })
        .collect()
}

/// Plain-text report over `entries` and `policy`'s statistics.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn report(policy: &GatePolicy, entries: &[LedgerEntry]) -> String {
    let mut out = format!(
        "gate policy {} (pass target {:.0}%, min samples {}, explore {:.0}%)\n",
        policy.version,
        policy.pass_target * 100.0,
        policy.min_samples,
        policy.explore_rate * 100.0
    );
    if entries.is_empty() {
        out.push_str("\nno gate attempts recorded yet\n");
        return out;
    }
    let mut spend: BTreeMap<Tier, (usize, f64)> = BTreeMap::new();
    let mut blocks: HashMap<(Option<&str>, &str, u64), (bool, u32)> = HashMap::new();
    for e in entries {
        let s = spend.entry(e.tier).or_default();
        s.0 += 1;
        s.1 += e.cost_usd;
        let b = blocks
            .entry((e.run_id.as_deref(), e.block.as_str(), e.at_ms))
            .or_default();
        b.0 = e.block_passed;
        b.1 = b.1.max(e.attempt);
    }
    let total: f64 = spend.values().map(|(_, c)| c).sum();
    let passed = blocks.values().filter(|(p, _)| *p).count();
    let escalated = blocks.values().filter(|(_, n)| *n > 1).count();
    let explored = entries.iter().filter(|e| e.explored).count();
    let _ = writeln!(
        out,
        "\n{} blocks, {} passed ({:.0}%), {} needed more than one attempt; {} attempts, {} explored; ${total:.4} spent",
        blocks.len(),
        passed,
        100.0 * passed as f64 / blocks.len().max(1) as f64,
        escalated,
        entries.len(),
        explored,
    );
    out.push_str("\nspend by tier:\n");
    for (tier, (n, cost)) in &spend {
        let _ = writeln!(
            out,
            "  {:<9} {n:>5} attempts  ${cost:>9.4}  ({:.0}%)",
            tier.as_str(),
            if total > 0.0 {
                100.0 * cost / total
            } else {
                0.0
            }
        );
    }
    let stats = bucket_stats(entries, &HashMap::new());
    out.push_str("\nfirst attempts by bucket (n, pass rate, lower bound, mean cost):\n");
    for (bucket, b) in &stats {
        let _ = writeln!(out, "  {bucket}");
        for (tier, s) in &b.tiers {
            let lower = wilson_lower(s.passes, s.attempts, policy.z);
            let _ = writeln!(
                out,
                "    {:<9} n={:<4} pass={:>5.1}%  lower={:>5.1}%  ${:.4}{}",
                tier.as_str(),
                s.attempts,
                100.0 * f64::from(s.passes) / f64::from(s.attempts.max(1)),
                100.0 * lower,
                s.mean_cost().unwrap_or_default(),
                if s.attempts >= policy.min_samples && lower >= policy.pass_target {
                    "  earned"
                } else {
                    ""
                }
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::VerifyLevel;

    fn entry(bucket: &str, tier: Tier, attempt: u32, passed: bool, run: &str) -> LedgerEntry {
        LedgerEntry {
            at_ms: 1,
            policy: "v0-rules".into(),
            run_id: Some(run.into()),
            block: "b".into(),
            bucket: bucket.into(),
            attempt,
            tier,
            model: tier.as_str().into(),
            reason: "rule".into(),
            explored: false,
            propensity: 1.0,
            passed,
            level: VerifyLevel::Tested,
            required: VerifyLevel::Tested,
            cost_usd: 0.01,
            input_tokens: 1,
            output_tokens: 1,
            failure: None,
            block_passed: passed,
        }
    }

    #[test]
    fn only_first_attempts_count_and_failed_runs_veto_passes() {
        let entries = vec![
            entry("edit", Tier::Fast, 1, true, "r1"),
            entry("edit", Tier::Fast, 1, true, "r2"),
            entry("edit", Tier::Standard, 2, true, "r2"),
            entry("edit", Tier::Fast, 1, false, "r3"),
        ];
        let outcomes = HashMap::from([("r2".to_string(), false)]);
        let stats = bucket_stats(&entries, &outcomes);
        let fast = stats["edit"].tiers[&Tier::Fast];
        assert_eq!(fast.attempts, 3);
        assert_eq!(fast.passes, 1);
        assert!(!stats["edit"].tiers.contains_key(&Tier::Standard));
    }

    #[test]
    fn fit_versions_and_earns() {
        let entries: Vec<LedgerEntry> = (0..60)
            .map(|i| entry("docs", Tier::Fast, 1, i != 0, &format!("r{i}")))
            .collect();
        let base = GatePolicy::default();
        let fitted = fit(&base, &entries, &HashMap::new(), "v1");
        assert_eq!(fitted.parent.as_deref(), Some(crate::RULES_VERSION));
        assert_eq!(earned_tiers(&fitted)["docs"], vec![Tier::Fast]);
        let text = report(&fitted, &entries);
        assert!(text.contains("earned"), "{text}");
        assert!(text.contains("60 attempts"), "{text}");
    }
}
