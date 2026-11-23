//! The replay: how the recorded turns went, and how Gate's picks now and
//! each model on its own would have done on them.

use super::{learned_view, line, render, section, HAIKU, SONNET, TURN_COST_USD};
use aura_core_types::{Difficulty, PromptCategory};
use aura_gate::lead::delegation::Delegation;
use aura_gate::route::stats::GateStats;
use aura_gate::route::Work;

#[test]
fn the_replay_weighs_gates_picks_and_each_model_on_the_recorded_turns() {
    let text = learned_view();
    assert!(text.contains("Replay of 17 recorded turns"), "{text}");
    assert_eq!(line(&text, "as recorded"), "as recorded 17 76% $0.020");
    assert_eq!(
        line(&text, "Gate's picks now"),
        "Gate's picks now 3 67% $0.020",
        "Gate now sends unrated questions and debugging to sonnet"
    );
    assert_eq!(
        line(&text, "all on claude-haiku-4-5"),
        "all on claude-haiku-4-5 (claude plan) 14 79% $0.020"
    );
    assert_eq!(
        line(&text, "all on claude-sonnet-4-6"),
        "all on claude-sonnet-4-6 (claude plan) 3 67% $0.020"
    );
}

/// Gate picks Haiku for easy questions, proven there, and Sonnet for the
/// others, so its picks match only the turns that went that way.
#[test]
fn gates_picks_now_follow_each_turns_difficulty() {
    let mut stats = GateStats::default();
    let rows = [
        (Some(Difficulty::Easy), HAIKU, (10, true)),
        (Some(Difficulty::Hard), HAIKU, (2, false)),
        (Some(Difficulty::Hard), SONNET, (3, true)),
        (None, SONNET, (1, true)),
        (None, HAIKU, (4, true)),
    ];
    for (difficulty, model, outcomes) in rows {
        let work = Work::turn(PromptCategory::Question).of(difficulty);
        stats.learn_costing(&work, model, None, outcomes, (TURN_COST_USD, 0.0));
    }
    let expected = [
        "Replay of 20 recorded turns, each weighted by how unlikely its route was; a policy is judged on the turns it would have routed the same way",
        "  policy                                  matched  success   avg cost",
        "  as recorded                                  20      90%     $0.020",
        "  Gate's picks now                             14     100%     $0.020",
        "  all on claude-haiku-4-5 (claude plan)        16      88%     $0.020",
        "  all on claude-sonnet-4-6 (claude plan)        4     100%     $0.020",
    ];
    let text = render(&stats, &[], &Delegation::default());
    assert_eq!(section(&text, "Replay"), expected.join("\n"));
}
