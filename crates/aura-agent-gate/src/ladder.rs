//! What to do after a failed attempt.
//!
//! First the same tier gets one more try with the failing check output
//! in hand, keeping its changes. If that fails too, the next stronger
//! tier starts over (from the pre-block files when they could be
//! saved) with the evidence of what went wrong. When the frontier tier
//! has failed, or the attempt budget is spent, the block goes back to
//! the coordinator to re-plan.

use crate::block::Attempt;
use crate::tier::{Tier, Tiers};

/// Next step for a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// The block passed or is out of options.
    Stop,
    /// Retry on the same tier, keeping the changes.
    Fix(Tier),
    /// Start over on a stronger tier.
    Escalate(Tier),
}

/// Decide the step after `attempts`, the block's attempts so far.
#[must_use]
pub fn next(attempts: &[Attempt], tiers: &Tiers, max_attempts: u32) -> Next {
    let Some(last) = attempts.last() else {
        return Next::Stop;
    };
    if last.passed || attempts.len() >= max_attempts as usize {
        return Next::Stop;
    }
    let tries_here = attempts.iter().filter(|a| a.tier == last.tier).count();
    let has_evidence = last.checks.iter().any(|c| !c.passed);
    if tries_here == 1 && has_evidence {
        return Next::Fix(last.tier);
    }
    tiers.stronger(last.tier).map_or(Next::Stop, Next::Escalate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{CheckOutcome, VerifyLevel};

    fn attempt(tier: Tier, passed: bool, failing_check: bool) -> Attempt {
        Attempt {
            n: 1,
            tier,
            model: tier.as_str().into(),
            effort: "medium".into(),
            reason: "rule".into(),
            explored: false,
            propensity: 1.0,
            passed,
            level: VerifyLevel::Ran,
            failure: (!passed).then(|| "failed".into()),
            checks: vec![CheckOutcome {
                stage: "check".into(),
                command: "t".into(),
                passed: !failing_check,
                exit_code: Some(i32::from(failing_check)),
                output: String::new(),
            }],
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            files_changed: Vec::new(),
            summary: String::new(),
            agent: None,
            wall_ms: 0,
        }
    }

    fn tiers() -> Tiers {
        Tiers::for_frontier("claude-opus-5-5")
    }

    #[test]
    fn fix_then_escalate_then_stop() {
        let t = tiers();
        let mut a = vec![attempt(Tier::Fast, false, true)];
        assert_eq!(next(&a, &t, 6), Next::Fix(Tier::Fast));
        a.push(attempt(Tier::Fast, false, true));
        assert_eq!(next(&a, &t, 6), Next::Escalate(Tier::Standard));
        a.push(attempt(Tier::Standard, false, false));
        assert_eq!(next(&a, &t, 6), Next::Escalate(Tier::Frontier));
        a.push(attempt(Tier::Frontier, false, true));
        assert_eq!(next(&a, &t, 6), Next::Fix(Tier::Frontier));
        a.push(attempt(Tier::Frontier, false, true));
        assert_eq!(next(&a, &t, 6), Next::Stop);
    }

    #[test]
    fn passing_or_budget_stops() {
        let t = tiers();
        assert_eq!(next(&[attempt(Tier::Fast, true, false)], &t, 4), Next::Stop);
        assert_eq!(next(&[attempt(Tier::Fast, false, true)], &t, 1), Next::Stop);
        assert_eq!(next(&[], &t, 4), Next::Stop);
    }

    #[test]
    fn flat_tiers_never_escalate() {
        let t = Tiers::for_frontier("local/qwen3:4b");
        let a = vec![attempt(Tier::Fast, false, false)];
        assert_eq!(next(&a, &t, 4), Next::Stop);
    }
}
