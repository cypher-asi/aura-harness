//! Which of a model's outcomes it is judged by for a piece of work.

use aura_core_types::{Difficulty, PromptCategory, RouteLevel, RouteReason};

use super::judged;
use crate::fixtures::{
    chosen, chosen_at, claude, no_exploring, priced_turn, turns, HAIKU, OPUS, SONNET,
};
use crate::stats::GateStats;
use crate::{pick, Work};

#[test]
fn a_subtask_kind_with_enough_outcomes_is_judged_on_its_own() {
    let category = PromptCategory::Debugging;
    let subtasks = |kind| Work::new(RouteLevel::Subtask, category, Some(kind));
    let mut stats = GateStats::default();
    stats.learn(&subtasks("explorer"), HAIKU, None, (60, true));
    stats.learn(&subtasks("rust-engineer"), HAIKU, None, (10, false));
    let routed = |kind| {
        let decision = pick(
            &stats,
            &Work::new(RouteLevel::Subtask, category, kind).of(Some(Difficulty::Easy)),
            &claude(),
            &no_exploring(),
            &mut || 0.0,
        )
        .unwrap();
        (decision.model.id, decision.reason)
    };

    let proven = (HAIKU.to_string(), RouteReason::Proven);
    assert_eq!(routed(None), proven, "60 of 70 overall");
    assert_eq!(routed(Some("explorer")), proven);
    assert_eq!(routed(Some("tests")), proven, "no outcomes of its own yet");
    let (model, reason) = routed(Some("rust-engineer"));
    assert_ne!(model, HAIKU, "0 of 10 for this kind rules it out");
    assert_eq!(reason, RouteReason::Default);

    let turn = pick(
        &stats,
        &Work::new(RouteLevel::Turn, category, Some("rust-engineer")),
        &claude(),
        &no_exploring(),
        &mut || 0.0,
    )
    .unwrap();
    assert_ne!(
        turn.reason,
        RouteReason::Proven,
        "subtasks teach turns nothing"
    );
}

#[test]
fn failures_of_led_turns_do_not_rule_the_lead_model_out() {
    let category = PromptCategory::MultiFileChange;
    let mut stats = GateStats::default();
    stats.learn_costing(
        &Work::turn(category),
        SONNET,
        Some(true),
        (3, false),
        (0.1, 0.1),
    );
    assert_eq!(chosen(&stats, category, &claude()).0, SONNET);
    turns(&mut stats, 10, category, SONNET, false);
    assert_eq!(
        chosen(&stats, category, &claude()).0,
        OPUS,
        "failing every turn of its own does"
    );
}

#[test]
fn a_model_is_judged_on_work_of_the_same_difficulty() {
    let category = PromptCategory::MultiFileChange;
    let work = |difficulty| Work::turn(category).of(Some(difficulty));
    let at = |stats: &GateStats, difficulty| chosen_at(stats, &work(difficulty), &claude()).0;
    let mut stats = GateStats::default();
    priced_turn(&mut stats, &work(Difficulty::Hard), OPUS, true, 1.0);
    stats.learn_costing(
        &work(Difficulty::Hard),
        SONNET,
        None,
        (10, false),
        (0.3, 0.0),
    );
    assert_eq!(at(&stats, Difficulty::Hard), OPUS);
    assert_eq!(
        at(&stats, Difficulty::Medium),
        OPUS,
        "with no medium turn to go by, the category's"
    );
    priced_turn(&mut stats, &work(Difficulty::Medium), SONNET, true, 0.1);
    assert_eq!(
        at(&stats, Difficulty::Medium),
        SONNET,
        "failing hard work says little of medium work"
    );
    assert_eq!(at(&stats, Difficulty::Hard), OPUS);
    assert_eq!(
        chosen(&stats, PromptCategory::Debugging, &claude()).0,
        SONNET,
        "nor of other work"
    );
}

#[test]
fn passing_easy_work_does_not_prove_a_model_on_hard_work() {
    let category = PromptCategory::MultiFileChange;
    let work = |difficulty| Work::turn(category).of(Some(difficulty));
    let mut stats = GateStats::default();
    stats.learn(&work(Difficulty::Easy), HAIKU, None, (10, true));
    stats.learn(&work(Difficulty::Hard), HAIKU, None, (1, false));
    let row = judged(&stats, &work(Difficulty::Hard), HAIKU, 10).unwrap();
    assert_eq!((row.tries, row.successes), (1, 0));
    let row = judged(&stats, &Work::turn(category), HAIKU, 10).unwrap();
    assert_eq!((row.tries, row.successes), (11, 10), "unrated: all of them");
}
