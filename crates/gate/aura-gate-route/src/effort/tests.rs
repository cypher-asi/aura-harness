//! The effort work starts at on each tier.

use aura_core_types::{Difficulty, PromptCategory, RouteLevel};
use aura_model::model_catalog::ModelTier;
use aura_model::ThinkingEffort;

use crate::fixtures::{claude, easy, no_exploring, no_fast_tier, HAIKU, SONNET};
use crate::stats::GateStats;
use crate::{pick, Candidate, Decision, Work};

/// Where `work` starts among `candidates` with nothing learned, without
/// exploring.
fn start(work: &Work<'_>, candidates: &[Candidate]) -> Decision {
    pick(
        &GateStats::default(),
        work,
        candidates,
        &no_exploring(),
        &mut || 0.0,
    )
    .unwrap()
}

#[test]
fn work_starts_at_its_difficultys_effort() {
    let effort = |difficulty, candidates: &[Candidate]| {
        start(
            &Work::turn(PromptCategory::MultiFileChange).of(difficulty),
            candidates,
        )
        .effort
    };
    assert_eq!(
        effort(Some(Difficulty::Easy), &no_fast_tier(&claude())),
        ThinkingEffort::Low
    );
    assert_eq!(effort(None, &claude()), ThinkingEffort::Medium);
    assert_eq!(
        effort(Some(Difficulty::Medium), &claude()),
        ThinkingEffort::Medium
    );
    assert_eq!(
        effort(Some(Difficulty::Hard), &claude()),
        ThinkingEffort::High
    );
}

#[test]
fn the_fast_tier_thinks_on_code_work_only() {
    for category in [
        PromptCategory::SmallChange,
        PromptCategory::MultiFileChange,
        PromptCategory::Debugging,
    ] {
        let decision = start(&easy(category), &claude());
        assert_eq!(
            (decision.model.id.as_str(), decision.effort),
            (HAIKU, ThinkingEffort::Medium),
            "{category}"
        );
    }
    for category in [
        PromptCategory::Question,
        PromptCategory::Research,
        PromptCategory::Writing,
        PromptCategory::Other,
    ] {
        let decision = start(&easy(category), &claude());
        assert_eq!(
            (decision.model.id.as_str(), decision.effort),
            (HAIKU, ThinkingEffort::Off),
            "{category}"
        );
        assert_eq!(
            decision.label(),
            "claude-haiku-4-5 (claude plan) with thinking off"
        );
        assert_eq!(decision.route(RouteLevel::Turn).effort, None);
    }
}

#[test]
fn work_off_the_fast_tier_or_harder_than_easy_keeps_its_difficultys_effort() {
    let below_standard: Vec<Candidate> = claude()
        .into_iter()
        .filter(|c| c.tier < ModelTier::Standard)
        .collect();
    let hard = start(
        &Work::turn(PromptCategory::Debugging).of(Some(Difficulty::Hard)),
        &below_standard,
    );
    assert_eq!(
        (hard.model.id.as_str(), hard.effort),
        (HAIKU, ThinkingEffort::High),
        "harder code work it gets keeps its effort"
    );
    let other = start(&easy(PromptCategory::Question), &no_fast_tier(&claude()));
    assert_eq!(
        (other.model.id.as_str(), other.effort),
        (SONNET, ThinkingEffort::Low),
        "other tiers keep the effort of the difficulty"
    );
}
