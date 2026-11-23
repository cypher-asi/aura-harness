//! [`choose_way`] on what Gate has learned of turns done both ways.

use super::{choose_way, Way};
use aura_config::GateConfig;
use aura_core_types::{Difficulty, PromptCategory};
use aura_gate_route::stats::GateStats;
use aura_gate_route::Work;

const SONNET: &str = "claude/claude-sonnet-4-6";
const OPUS: &str = "claude/claude-opus-4-7";

/// Turns the model led, handing the work to subtasks.
const LED: Option<bool> = Some(true);
/// Turns the model did the work of itself.
const DIRECT: Option<bool> = Some(false);

/// A roll that keeps the best known way.
const KEPT: f64 = 0.99;
/// A roll that takes the other way, where exploring may.
const EXPLORED: f64 = 0.0;

fn settings(explore_rate: f64) -> GateConfig {
    GateConfig {
        explore_rate,
        ..GateConfig::defaults()
    }
}

/// How a turn on Sonnet asking for `category`, of unknown difficulty, is
/// done given `stats`, exploring at the default rate with `roll`.
fn way(stats: &GateStats, category: PromptCategory, roll: f64) -> Way {
    choose_way(
        stats,
        (category, None),
        SONNET,
        &settings(0.05),
        &mut || roll,
    )
}

#[test]
fn with_nothing_learned_the_model_does_the_work() {
    let nothing = GateStats::default();
    for category in PromptCategory::CLASSES {
        assert!(!way(&nothing, category, KEPT).led, "{category}");
    }
    let tried = way(&nothing, PromptCategory::MultiFileChange, EXPLORED);
    assert_eq!((tried.led, tried.explored), (true, true));
    let never = way(&nothing, PromptCategory::Question, EXPLORED);
    assert_eq!((never.led, never.explored), (false, false));
}

#[test]
fn a_frontier_model_leads_only_once_leading_is_proven_cheaper() {
    let category = PromptCategory::Planning;
    let on_opus = |stats: &GateStats, roll: f64| {
        choose_way(stats, (category, None), OPUS, &settings(0.05), &mut || roll)
    };
    let empty = GateStats::default();
    let kept = on_opus(&empty, EXPLORED);
    assert_eq!((kept.led, kept.explored), (false, false), "never explored");
    assert!(!on_opus(&empty, KEPT).led);
}

#[test]
fn of_two_proven_ways_the_cheaper_in_total_wins() {
    let changes = Work::turn(PromptCategory::MultiFileChange);
    let mut stats = GateStats::default();
    stats.learn_costing(&changes, SONNET, LED, (10, true), (0.10, 0.50));
    stats.learn_costing(&changes, SONNET, DIRECT, (10, true), (0.40, 0.0));
    assert!(!way(&stats, changes.category, KEPT).led, "0.40 beats 0.60");

    let mut stats = GateStats::default();
    stats.learn_costing(&changes, SONNET, LED, (10, true), (0.10, 0.20));
    stats.learn_costing(&changes, SONNET, DIRECT, (10, true), (0.40, 0.0));
    assert!(way(&stats, changes.category, KEPT).led, "0.30 beats 0.40");
}

#[test]
fn proven_leading_waits_until_working_directly_is_proven_or_ruled_out() {
    let debugging = Work::turn(PromptCategory::Debugging);
    let mut stats = GateStats::default();
    stats.learn_costing(&debugging, SONNET, LED, (10, true), (0.1, 0.1));
    assert!(
        !way(&stats, debugging.category, KEPT).led,
        "only the led way is proven: working directly gets its tries"
    );
    stats.learn_costing(&debugging, SONNET, DIRECT, (9, true), (0.4, 0.0));
    assert!(!way(&stats, debugging.category, KEPT).led, "not proven yet");
    stats.learn_costing(&debugging, SONNET, DIRECT, (1, true), (0.4, 0.0));
    assert!(way(&stats, debugging.category, KEPT).led, "0.20 beats 0.40");

    let mut stats = GateStats::default();
    stats.learn_costing(&debugging, SONNET, LED, (10, true), (1.0, 1.0));
    stats.learn_costing(&debugging, SONNET, DIRECT, (3, false), (0.1, 0.0));
    assert!(
        way(&stats, debugging.category, KEPT).led,
        "working directly is ruled out"
    );

    let changes = Work::turn(PromptCategory::MultiFileChange);
    let mut stats = GateStats::default();
    stats.learn_costing(&changes, SONNET, DIRECT, (3, false), (0.1, 0.0));
    assert!(
        way(&stats, changes.category, KEPT).led,
        "3 failures rule working directly out"
    );
    stats.learn_costing(&changes, SONNET, LED, (3, false), (0.1, 0.1));
    assert!(
        !way(&stats, changes.category, KEPT).led,
        "both ruled out: the model does the work"
    );
}

#[test]
fn leading_proven_cheaper_on_easy_work_says_nothing_of_hard_work() {
    let changes = Work::turn(PromptCategory::MultiFileChange);
    let easy = changes.of(Some(Difficulty::Easy));
    let hard = changes.of(Some(Difficulty::Hard));
    let mut stats = GateStats::default();
    stats.learn_costing(&easy, SONNET, LED, (10, true), (0.06, 0.03));
    stats.learn_costing(&hard, SONNET, DIRECT, (10, true), (0.50, 0.0));
    let led = |stats: &GateStats, work: Work<'_>| {
        choose_way(
            stats,
            (work.category, work.difficulty),
            SONNET,
            &settings(0.0),
            &mut || KEPT,
        )
        .led
    };
    assert!(
        !led(&stats, hard),
        "nothing proves leading hard work cheaper"
    );
    assert!(
        led(&stats, changes),
        "over every turn, leading looks cheaper"
    );

    stats.learn_costing(&hard, SONNET, LED, (10, true), (0.30, 0.40));
    assert!(
        !led(&stats, hard),
        "led hard work costs 0.70 in all, done directly 0.50"
    );
}

#[test]
fn exploring_never_takes_a_ruled_out_way() {
    let research = Work::turn(PromptCategory::Research);
    let mut stats = GateStats::default();
    stats.learn_costing(&research, SONNET, LED, (3, false), (0.1, 0.1));
    let way = way(&stats, research.category, EXPLORED);
    assert_eq!((way.led, way.explored), (false, false));
}
