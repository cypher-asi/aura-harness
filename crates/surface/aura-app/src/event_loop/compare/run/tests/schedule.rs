//! What the user's chat hears as each run ends, and what the record
//! notes about the budget.

use super::super::schedule::{budget_note, progress, spent_of_budget, unpriced_arm_note};
use aura_core_types::{AgentId, CompareArm, CompareCopy, CompareRun, ModelTokens};
use aura_gate::compare::Spent;
use std::path::PathBuf;

/// Run `a-1` as it ended: in 3m 05s, at $0.004, its check as
/// `check_passed` says, and stopped by `error` when there is one.
fn ended(check_passed: Option<bool>, error: Option<&str>) -> CompareRun {
    CompareRun {
        name: "a-1".to_string(),
        arm: CompareArm::A,
        agent_id: AgentId::new([1; 32]),
        prompt_tx: None,
        model: "aura-claude-sonnet-4-6".to_string(),
        copy: CompareCopy {
            root: PathBuf::new(),
            workdir: PathBuf::new(),
            git_dir: None,
            baseline: String::new(),
        },
        wall_ms: 185_000,
        cost_usd: 0.004,
        tokens: Vec::new(),
        plan_usage: Vec::new(),
        turns: Vec::new(),
        check_passed,
        files_changed: 0,
        error: error.map(str::to_string),
        task: None,
    }
}

#[test]
fn the_chat_hears_how_each_run_ended() {
    let stopped = Some("opening the run: no session");
    for (check, error, line) in [
        (
            Some(true),
            None,
            "Compare #1: a-1 done in 3m 05s, $0.0040, check passed",
        ),
        (
            Some(false),
            None,
            "Compare #1: a-1 done in 3m 05s, $0.0040, check failed",
        ),
        (None, None, "Compare #1: a-1 done in 3m 05s, $0.0040"),
        (
            None,
            stopped,
            "Compare #1: a-1 stopped: opening the run: no session",
        ),
    ] {
        assert_eq!(progress("Compare #1", &ended(check, error)), line);
    }
}

/// Spent `usd` on priced calls, leaving out `unpriced_calls`.
const fn spent(usd: f64, unpriced_calls: u32) -> Spent {
    Spent {
        usd,
        unpriced_calls,
    }
}

#[test]
fn spend_against_the_budget_marks_and_explains_unpriced_calls() {
    for (spent, line) in [
        (spent(1.2, 0), "spent $1.20 of the $2.00 budget"),
        (
            spent(1.2, 14),
            "spent $1.20+? of the $2.00 budget; 14 calls have no known price and are not counted",
        ),
        (
            spent(1.2, 1),
            "spent $1.20+? of the $2.00 budget; 1 call has no known price and is not counted",
        ),
        (
            spent(0.0, 3),
            "spent unknown of the $2.00 budget; 3 calls have no known price and are not counted",
        ),
    ] {
        assert_eq!(spent_of_budget(2.0, spent), line);
    }
}

#[test]
fn the_budget_note_says_what_was_spent_and_what_it_could_not_count() {
    assert_eq!(
        budget_note(Some(2.0), spent(2.1, 0), 4, false).as_deref(),
        Some("4 runs did not start: spent $2.10 of the $2.00 budget")
    );
    assert_eq!(
        budget_note(Some(2.0), spent(2.1, 3), 1, false).as_deref(),
        Some(
            "1 run did not start: spent $2.10+? of the $2.00 budget; \
             3 calls have no known price and are not counted"
        ),
        "the budget still stops on what the priced calls cost"
    );
    assert_eq!(
        budget_note(Some(2.0), spent(0.0, 14), 0, false).as_deref(),
        Some(
            "The budget counts only priced calls: spent unknown of the $2.00 budget; \
             14 calls have no known price and are not counted"
        ),
        "a budget unpriced calls never reach is not silent"
    );
    assert_eq!(budget_note(Some(2.0), spent(1.0, 0), 0, false), None);
    assert_eq!(budget_note(Some(2.0), spent(2.1, 3), 4, true), None);
    assert_eq!(budget_note(None, spent(2.1, 3), 0, false), None);
}

/// Run `b-1`, ended having made `calls` calls on a model `mystery`,
/// `unpriced_calls` of them with no known price.
fn ended_with_calls(calls: u32, unpriced_calls: u32) -> CompareRun {
    let mut run = ended(None, None);
    run.name = "b-1".to_string();
    run.arm = CompareArm::B;
    run.cost_usd = 0.0;
    run.tokens = vec![ModelTokens {
        model: "mystery".to_string(),
        calls,
        unpriced_calls,
        ..ModelTokens::default()
    }];
    run
}

#[test]
fn the_chat_hears_when_none_of_an_arms_calls_had_a_price() {
    assert_eq!(
        unpriced_arm_note("Compare #1", Some(2.0), &ended_with_calls(14, 14)).as_deref(),
        Some(
            "Compare #1: none of b-1's 14 calls had a known price, \
             so the $2.00 budget does not count arm B"
        )
    );
    assert_eq!(
        unpriced_arm_note("Compare #1", Some(2.0), &ended_with_calls(14, 3)),
        None,
        "some calls were priced"
    );
    assert_eq!(
        unpriced_arm_note("Compare #1", Some(2.0), &ended_with_calls(0, 0)),
        None,
        "no calls"
    );
    assert_eq!(
        unpriced_arm_note("Compare #1", None, &ended_with_calls(14, 14)),
        None,
        "no budget"
    );
}
