//! Proven picks, exploring, and the models a lead's subtasks go to.

use aura_config::GateConfig;
use aura_core_types::{Difficulty, OutcomeSignal, PromptCategory, Route, RouteLevel, RouteReason};
use aura_model::model_catalog::ModelTier;

use super::{below_lead, pick};
use crate::candidate::is_current;
use crate::fixtures::{
    chosen, chosen_at, claude, easy, new_sonnet_failing, turns, versions, HAIKU, LOCAL, OPUS,
    SONNET,
};
use crate::stats::GateStats;
use crate::{escalate, Candidate, SplitMix64, Work};

#[test]
fn a_model_becomes_the_pick_after_min_tries_successes() {
    let mut stats = GateStats::default();
    let question = easy(PromptCategory::Question);
    turns(&mut stats, 9, PromptCategory::Question, LOCAL, true);
    assert_eq!(
        chosen_at(&stats, &question, &claude()),
        (HAIKU.to_string(), RouteReason::Default),
        "nine tries are not enough"
    );
    turns(&mut stats, 1, PromptCategory::Question, LOCAL, true);
    assert_eq!(
        chosen_at(&stats, &question, &claude()),
        (LOCAL.to_string(), RouteReason::Proven)
    );
    assert_eq!(
        chosen_at(&stats, &easy(PromptCategory::Writing), &claude()).0,
        HAIKU,
        "proven on questions says nothing about writing"
    );
    assert_eq!(
        chosen(&stats, PromptCategory::Question, &claude()).0,
        SONNET,
        "nor about work that is not easy"
    );
}

#[test]
fn a_proven_model_drops_back_after_failures() {
    let mut stats = GateStats::default();
    let question = easy(PromptCategory::Question);
    turns(&mut stats, 10, PromptCategory::Question, LOCAL, true);
    turns(&mut stats, 1, PromptCategory::Question, LOCAL, false);
    assert_eq!(
        chosen_at(&stats, &question, &claude()),
        (LOCAL.to_string(), RouteReason::Proven),
        "10 of 11 is still above 85%"
    );
    turns(&mut stats, 1, PromptCategory::Question, LOCAL, false);
    assert_eq!(
        chosen_at(&stats, &question, &claude()),
        (HAIKU.to_string(), RouteReason::Default),
        "10 of 12 is below 85%"
    );
}

#[test]
fn the_cheapest_proven_model_wins() {
    let mut stats = GateStats::default();
    turns(&mut stats, 10, PromptCategory::Debugging, SONNET, true);
    assert_eq!(
        chosen(&stats, PromptCategory::Debugging, &claude()),
        (SONNET.to_string(), RouteReason::Proven)
    );
    turns(&mut stats, 10, PromptCategory::Debugging, HAIKU, true);
    assert_eq!(
        chosen(&stats, PromptCategory::Debugging, &claude()).0,
        SONNET,
        "unrated work starts no lower than the standard tier"
    );
    let debugging = easy(PromptCategory::Debugging);
    assert_eq!(chosen_at(&stats, &debugging, &claude()).0, HAIKU);
    turns(&mut stats, 10, PromptCategory::Debugging, LOCAL, true);
    assert_eq!(chosen_at(&stats, &debugging, &claude()).0, LOCAL);
}

#[test]
fn a_proven_stronger_model_does_not_outrank_a_cheaper_start() {
    let mut stats = GateStats::default();
    turns(&mut stats, 10, PromptCategory::Debugging, OPUS, true);
    assert_eq!(
        chosen(&stats, PromptCategory::Debugging, &claude()),
        (SONNET.to_string(), RouteReason::Default)
    );
    turns(&mut stats, 2, PromptCategory::Debugging, SONNET, false);
    assert_eq!(
        chosen(&stats, PromptCategory::Debugging, &claude()),
        (OPUS.to_string(), RouteReason::Proven)
    );
}

#[test]
fn exploring_skips_ruled_out_models_until_nothing_else_is_cheaper() {
    let settings = GateConfig::defaults();
    let debugging = easy(PromptCategory::Debugging);
    let explored = |stats: &GateStats| {
        let decision = pick(stats, &debugging, &claude(), &settings, &mut || 0.0).unwrap();
        assert_eq!(decision.reason, RouteReason::Explore);
        decision.model.id
    };
    let mut stats = GateStats::default();
    turns(&mut stats, 2, PromptCategory::Debugging, HAIKU, false);
    assert_eq!(chosen_at(&stats, &debugging, &claude()).0, SONNET);
    assert_eq!(
        explored(&stats),
        LOCAL,
        "Haiku is ruled out, the local model is not"
    );
    turns(&mut stats, 2, PromptCategory::Debugging, LOCAL, false);
    assert_eq!(
        explored(&stats),
        HAIKU,
        "with every cheaper model ruled out, the nearest gets another try"
    );
    turns(&mut stats, 12, PromptCategory::Debugging, HAIKU, true);
    assert_eq!(
        chosen_at(&stats, &debugging, &claude()),
        (HAIKU.to_string(), RouteReason::Proven),
        "and comes back once those tries prove it"
    );
    let unrated = Work::turn(PromptCategory::Debugging);
    assert_eq!(
        pick(&stats, &unrated, &claude(), &settings, &mut || 0.0)
            .unwrap()
            .model
            .id,
        SONNET,
        "work that is not easy is never explored below the standard tier"
    );
}

#[test]
fn exploring_takes_the_next_cheaper_model() {
    let empty = GateStats::default();
    let settings = GateConfig::defaults();
    let debugging = easy(PromptCategory::Debugging);
    let explored = pick(&empty, &debugging, &claude(), &settings, &mut || 0.0).unwrap();
    assert_eq!(explored.model.id, LOCAL);
    assert_eq!(explored.reason, RouteReason::Explore);
    assert!(explored.explored);
    assert_eq!(explored.propensity, settings.explore_rate);

    let kept = pick(&empty, &debugging, &claude(), &settings, &mut || 0.99).unwrap();
    assert_eq!(kept.model.id, HAIKU);
    assert!(!kept.explored);
    assert_eq!(kept.propensity, 1.0 - settings.explore_rate);
    assert_eq!(
        kept.route(RouteLevel::Subtask),
        Route {
            level: RouteLevel::Subtask,
            model: HAIKU.to_string(),
            reason: RouteReason::Default,
            explored: false,
            propensity: 1.0 - settings.explore_rate,
            effort: Some("medium".to_string()),
        }
    );
}

#[test]
fn exploration_happens_at_about_the_configured_rate() {
    let empty = GateStats::default();
    let settings = GateConfig {
        explore_rate: 0.2,
        ..GateConfig::defaults()
    };
    let mut rng = SplitMix64::seeded(7);
    let draws = 10_000;
    let question = easy(PromptCategory::Question);
    let explored = (0..draws)
        .filter(|_| {
            pick(&empty, &question, &claude(), &settings, &mut rng)
                .unwrap()
                .explored
        })
        .count();
    let rate = f64::from(u32::try_from(explored).unwrap()) / f64::from(draws);
    assert!((rate - 0.2).abs() < 0.02, "explored {rate}");
}

#[test]
fn nothing_cheaper_means_no_exploring() {
    let mut stats = GateStats::default();
    turns(&mut stats, 10, PromptCategory::Question, LOCAL, true);
    let settings = GateConfig {
        explore_rate: 1.0,
        ..GateConfig::defaults()
    };
    let question = easy(PromptCategory::Question);
    let decision = pick(&stats, &question, &claude(), &settings, &mut || 0.0).unwrap();
    assert_eq!(decision.model.id, LOCAL);
    assert_eq!(decision.reason, RouteReason::Proven);
    assert!(!decision.explored);
    assert_eq!(decision.propensity, 1.0);
}

#[test]
fn exploring_never_picks_an_older_version() {
    let all = versions();
    let stats = new_sonnet_failing();
    let mut rng = SplitMix64::seeded(11);
    let exploring = GateConfig {
        explore_rate: 1.0,
        ..GateConfig::defaults()
    };
    for difficulty in [None, Some(Difficulty::Easy), Some(Difficulty::Hard)] {
        for category in [PromptCategory::MultiFileChange, PromptCategory::Research] {
            let work = Work::turn(category).of(difficulty);
            for _ in 0..50 {
                let model = pick(&stats, &work, &all, &exploring, &mut rng)
                    .unwrap()
                    .model;
                assert!(is_current(&model, &all), "{} was explored", model.id);
            }
        }
    }
}

#[test]
fn a_frontier_lead_keeps_its_subtasks_below_its_tier() {
    let ids = |among: Vec<Candidate>| -> Vec<String> { among.into_iter().map(|c| c.id).collect() };
    let claude = claude();
    assert_eq!(
        ids(below_lead(&claude, Some(OPUS))),
        [LOCAL, HAIKU, SONNET],
        "nothing of the lead's tier"
    );
    let among = below_lead(&claude, Some(OPUS));
    let empty = GateStats::default();
    assert_eq!(
        chosen(&empty, PromptCategory::Planning, &among).0,
        SONNET,
        "work for the strongest model goes to the strongest left"
    );
    let failed = (SONNET, None);
    let next = escalate(
        &empty,
        RouteLevel::Subtask,
        failed,
        OutcomeSignal::ChecksFailed,
        &claude,
    );
    assert_eq!(
        next.map(|d| d.model.id),
        Some(OPUS.to_string()),
        "a failed attempt may still go up to the lead's tier"
    );

    assert_eq!(below_lead(&claude, Some(SONNET)), claude);
    assert_eq!(below_lead(&claude, Some(HAIKU)), claude);
    assert_eq!(below_lead(&claude, None), claude, "an unknown lead");
    assert_eq!(
        below_lead(&claude, Some("claude/claude-opus-5-5")).len(),
        3,
        "a lead outside the candidates is placed by its name"
    );

    let no_standard: Vec<Candidate> = claude
        .iter()
        .filter(|c| c.tier != ModelTier::Standard)
        .cloned()
        .collect();
    assert_eq!(
        below_lead(&no_standard, Some(OPUS)),
        no_standard,
        "without a standard model the work stays where it was"
    );
}
