//! One arm of a comparison: its runs' medians and totals, its checks,
//! and how the judge rated it.

use super::median;
use crate::plan_shares::merge_plan_shares;
use crate::tokens::merge_tokens;
use aura_core_types::{CompareArm, ComparePayload, ModelTokens, PlanShare, RefVerdict};

/// One arm of a comparison, as the view shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmSummary {
    /// The arm.
    pub arm: CompareArm,
    /// Runs that started.
    pub runs: usize,
    /// Runs that ended without an error.
    pub finished: usize,
    /// Median wall time of the finished runs.
    pub median_wall_ms: Option<u64>,
    /// What every run cost.
    pub total_cost_usd: f64,
    /// Median cost of the finished runs.
    pub median_cost_usd: Option<f64>,
    /// Tokens per model over every run, most expensive first.
    pub tokens: Vec<ModelTokens>,
    /// Share of each plan window the arm's runs used.
    pub plan_usage: Vec<PlanShare>,
    /// Runs whose check passed.
    pub checks_passed: usize,
    /// Runs whose check ran.
    pub checks_run: usize,
    /// Pairs the judge gave this arm.
    pub judge_wins: usize,
    /// Pairs the judge gave the other arm.
    pub judge_losses: usize,
    /// Pairs the judge called even.
    pub judge_ties: usize,
    /// Runs the judge rated better than the reference.
    pub ref_better: usize,
    /// Runs the judge rated as good as the reference.
    pub ref_as_good: usize,
    /// Runs the judge rated worse than the reference.
    pub ref_worse: usize,
}

/// What `payload` says about `arm`.
#[must_use]
pub fn summarize(payload: &ComparePayload, arm: CompareArm) -> ArmSummary {
    let runs: Vec<_> = payload.arm_runs(arm).collect();
    let finished: Vec<_> = runs.iter().filter(|run| run.error.is_none()).collect();
    #[allow(clippy::cast_precision_loss)]
    let walls: Vec<f64> = finished.iter().map(|run| run.wall_ms as f64).collect();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let median_wall_ms = median(&walls).map(|ms| ms.round() as u64);
    let costs: Vec<f64> = finished.iter().map(|run| run.cost_usd).collect();
    let judged: Vec<_> = payload.judgments.iter().filter(|j| j.parsed).collect();
    let judged_as = |verdict: RefVerdict| {
        judged
            .iter()
            .filter(|j| j.vs_reference(arm) == Some(verdict))
            .count()
    };
    let won_by = |winner: Option<CompareArm>| judged.iter().filter(|j| j.winner == winner).count();
    ArmSummary {
        arm,
        runs: runs.len(),
        finished: finished.len(),
        median_wall_ms,
        total_cost_usd: runs.iter().map(|run| run.cost_usd).sum(),
        median_cost_usd: median(&costs),
        tokens: merge_tokens(runs.iter().flat_map(|run| run.tokens.iter())),
        plan_usage: merge_plan_shares(runs.iter().flat_map(|run| run.plan_usage.iter())),
        checks_passed: runs.iter().filter(|r| r.check_passed == Some(true)).count(),
        checks_run: runs.iter().filter(|r| r.check_passed.is_some()).count(),
        judge_wins: won_by(Some(arm)),
        judge_losses: won_by(Some(arm.opposite())),
        judge_ties: won_by(None),
        ref_better: judged_as(RefVerdict::Better),
        ref_as_good: judged_as(RefVerdict::AsGood),
        ref_worse: judged_as(RefVerdict::Worse),
    }
}
