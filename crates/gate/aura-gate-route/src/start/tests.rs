//! Where work starts, and when it starts a tier higher.

use aura_config::GateConfig;
use aura_core_auth::ProviderKind;
use aura_core_types::{Difficulty, PromptCategory, RouteLevel, RouteReason};
use aura_model::model_catalog::ModelTier;

use super::start_tier;
use crate::fixtures::{
    all, chosen, chosen_at, claude, easy, new_sonnet_failing, no_exploring, no_fast_tier,
    priced_turn, signed_in, turns, versions, with_local, HAIKU, LOCAL, NEW_OPUS, OPUS, SONNET,
};
use crate::stats::GateStats;
use crate::{candidates, pick, Candidate, SplitMix64, Work};

#[test]
fn difficulty_moves_where_work_starts() {
    use Difficulty::{Easy, Hard, Medium};
    use PromptCategory::{Planning, Question, SmallChange};
    assert_eq!(start_tier(Question, Some(Easy)), ModelTier::Fast);
    assert_eq!(start_tier(Question, Some(Medium)), ModelTier::Standard);
    assert_eq!(start_tier(SmallChange, Some(Easy)), ModelTier::Fast);
    for difficulty in [None, Some(Medium), Some(Hard)] {
        assert_eq!(
            start_tier(SmallChange, difficulty),
            ModelTier::Standard,
            "{difficulty:?}"
        );
    }
    assert_eq!(
        start_tier(Planning, Some(Easy)),
        ModelTier::Standard,
        "a plan is never easy enough for the fast tier"
    );

    let empty = GateStats::default();
    let start =
        |category, difficulty| chosen_at(&empty, &Work::turn(category).of(difficulty), &claude()).0;
    assert_eq!(start(Question, Some(Easy)), HAIKU);
    assert_eq!(start(Question, None), SONNET);
    assert_eq!(start(SmallChange, Some(Easy)), HAIKU);
    assert_eq!(start(SmallChange, Some(Medium)), SONNET);
    assert_eq!(start(SmallChange, Some(Hard)), SONNET);
}

#[test]
fn planning_starts_on_the_standard_tier() {
    let empty = GateStats::default();
    assert_eq!(
        chosen(&empty, PromptCategory::Planning, &claude()),
        (SONNET.to_string(), RouteReason::Default)
    );
    assert_eq!(chosen(&empty, PromptCategory::Planning, &all()).0, SONNET);
}

#[test]
fn only_easy_work_starts_on_the_cheapest_fast_model() {
    let empty = GateStats::default();
    let small = easy(PromptCategory::SmallChange);
    assert_eq!(
        chosen_at(&empty, &small, &all()),
        (HAIKU.to_string(), RouteReason::Default)
    );
    assert_eq!(
        chosen_at(&empty, &small, &no_fast_tier(&claude())).0,
        SONNET,
        "the next tier up when there is no fast model"
    );
    for difficulty in [None, Some(Difficulty::Medium), Some(Difficulty::Hard)] {
        assert_eq!(
            chosen_at(&empty, &small.of(difficulty), &all()),
            (SONNET.to_string(), RouteReason::Default),
            "{difficulty:?}"
        );
    }
}

#[test]
fn a_fast_model_that_fails_too_often_hands_the_work_up_a_tier() {
    let category = PromptCategory::SmallChange;
    let small = easy(category);
    let mut stats = GateStats::default();
    assert_eq!(chosen_at(&stats, &small, &claude()).0, HAIKU);
    turns(&mut stats, 1, category, HAIKU, false);
    assert_eq!(
        chosen_at(&stats, &small, &claude()).0,
        HAIKU,
        "nine of ten can still pass"
    );
    turns(&mut stats, 1, category, HAIKU, false);
    assert_eq!(
        chosen_at(&stats, &small, &claude()),
        (SONNET.to_string(), RouteReason::Default),
        "eight of ten cannot reach 85%"
    );
    assert_eq!(
        chosen_at(&stats, &easy(PromptCategory::Question), &claude()).0,
        HAIKU,
        "failing at small changes says nothing about other work"
    );
    turns(&mut stats, 2, category, SONNET, false);
    assert_eq!(
        chosen_at(&stats, &small, &claude()).0,
        SONNET,
        "two failures do not send a standard model's work to the frontier"
    );
    turns(&mut stats, 8, category, SONNET, false);
    assert_eq!(chosen_at(&stats, &small, &claude()).0, OPUS);
    turns(&mut stats, 10, category, OPUS, false);
    assert_eq!(
        chosen_at(&stats, &small, &claude()).0,
        OPUS,
        "the strongest model when every tier is sent up"
    );
}

/// Hard debugging that costs $0.35 a turn on Sonnet and $1.00 on
/// Opus: Sonnet's failures go on to Opus, so starting on Sonnet
/// costs `0.35 + (1 - rate) * 1.00` against `1.00`, and is the
/// cheaper start while more than 35% of its turns pass.
#[test]
fn a_standard_model_keeps_its_work_until_starting_on_it_costs_more() {
    let work = Work::turn(PromptCategory::Debugging).of(Some(Difficulty::Hard));
    let picked = |passes: usize, failures: usize| {
        let mut stats = GateStats::default();
        priced_turn(&mut stats, &work, OPUS, true, 1.0);
        stats.learn_costing(&work, SONNET, None, (passes, true), (0.35, 0.0));
        stats.learn_costing(&work, SONNET, None, (failures, false), (0.35, 0.0));
        chosen_at(&stats, &work, &claude())
    };
    let sonnet = (SONNET.to_string(), RouteReason::Default);
    let opus = (OPUS.to_string(), RouteReason::Default);
    assert_eq!(picked(0, 2), sonnet, "eight more could still pass");
    assert_eq!(picked(5, 5), sonnet, "half fail, and it is still cheaper");
    assert_eq!(picked(4, 6), sonnet);
    assert_eq!(picked(3, 7), opus, "0.35 + 0.7 * 1.00 is more than 1.00");
    assert_eq!(
        picked(0, 8),
        opus,
        "two passes more would not make it cheaper"
    );
    assert_eq!(picked(6, 14), opus);
    assert_eq!(picked(8, 12), sonnet);
}

#[test]
fn unrated_work_starts_on_the_cheapest_standard_model() {
    let empty = GateStats::default();
    for category in [
        PromptCategory::SmallChange,
        PromptCategory::Question,
        PromptCategory::MultiFileChange,
        PromptCategory::Debugging,
        PromptCategory::Research,
        PromptCategory::Writing,
        PromptCategory::Other,
    ] {
        assert_eq!(
            chosen(&empty, category, &all()),
            (SONNET.to_string(), RouteReason::Default),
            "{category}"
        );
    }
}

#[test]
fn without_a_standard_model_every_category_gets_the_strongest() {
    let chatgpt = candidates(&with_local(), signed_in(&[ProviderKind::Chatgpt]), |_| {
        false
    });
    let empty = GateStats::default();
    for category in [PromptCategory::Question, PromptCategory::Debugging] {
        assert_eq!(chosen(&empty, category, &chatgpt).0, "chatgpt/gpt-5.4");
    }
    let local = candidates(&with_local(), signed_in(&[]), |_| false);
    assert_eq!(chosen(&empty, PromptCategory::Planning, &local).0, LOCAL);
    assert!(pick(
        &empty,
        &Work::new(RouteLevel::Turn, PromptCategory::Question, None),
        &[],
        &no_exploring(),
        &mut || 0.0
    )
    .is_none());
}

/// Twelve hard multi-file turns that Haiku passed.
fn passed_on_haiku() -> GateStats {
    let work = Work::turn(PromptCategory::MultiFileChange).of(Some(Difficulty::Hard));
    let mut stats = GateStats::default();
    stats.learn(&work, HAIKU, None, (12, true));
    stats
}

#[test]
fn work_that_is_not_easy_never_starts_below_the_standard_tier() {
    let stats = passed_on_haiku();
    let work = Work::turn(PromptCategory::MultiFileChange);
    let start = |candidates: &[Candidate], work: &Work<'_>| chosen_at(&stats, work, candidates).0;
    for difficulty in [None, Some(Difficulty::Medium), Some(Difficulty::Hard)] {
        assert_eq!(
            start(&claude(), &work.of(difficulty)),
            SONNET,
            "not even where it is proven: {difficulty:?}"
        );
    }
    assert_eq!(
        start(&claude(), &work.of(Some(Difficulty::Easy))),
        HAIKU,
        "easy work still starts cheap"
    );
    let below_standard: Vec<Candidate> = claude()
        .into_iter()
        .filter(|c| c.tier < ModelTier::Standard)
        .collect();
    assert_eq!(
        start(&below_standard, &work.of(Some(Difficulty::Hard))),
        HAIKU,
        "with nothing stronger it starts where it can"
    );
}

#[test]
fn exploring_never_takes_work_that_is_not_easy_below_the_standard_tier() {
    let stats = passed_on_haiku();
    let work = Work::turn(PromptCategory::MultiFileChange);
    let mut rng = SplitMix64::seeded(5);
    let settings = GateConfig {
        explore_rate: 1.0,
        ..GateConfig::defaults()
    };
    for difficulty in [None, Some(Difficulty::Medium), Some(Difficulty::Hard)] {
        for _ in 0..200 {
            let decision =
                pick(&stats, &work.of(difficulty), &claude(), &settings, &mut rng).unwrap();
            assert!(
                decision.model.tier >= ModelTier::Standard,
                "exploring stays at or above the floor: {decision:?}"
            );
        }
    }
}

#[test]
fn work_goes_up_a_tier_rather_than_back_to_an_older_version() {
    // The newer sonnet keeps failing: the work does not fall to the
    // older one, which nothing is known against and which lists
    // at the same price.
    let sonnets_and_opus: Vec<Candidate> = versions()
        .into_iter()
        .filter(|c| c.tier >= ModelTier::Standard)
        .collect();
    let work = Work::turn(PromptCategory::MultiFileChange).of(Some(Difficulty::Medium));
    assert_eq!(
        chosen_at(&new_sonnet_failing(), &work, &sonnets_and_opus).0,
        NEW_OPUS,
        "up a tier, not back a version"
    );
}
