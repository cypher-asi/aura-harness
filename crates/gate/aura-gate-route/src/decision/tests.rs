//! The route and label decisions record, held byte for byte.

use aura_config::GateConfig;
use aura_core_types::{OutcomeSignal, PromptCategory, RouteLevel};
use aura_model::ThinkingEffort;

use super::Decision;
use crate::fixtures::{all, claude, easy, no_exploring, HAIKU, OPUS};
use crate::stats::GateStats;
use crate::{escalate, pick};

/// The route each of [`decisions`] records on its prompt, and its label,
/// byte for byte.
const PINNED: [(&str, &str); 5] = [
    (
        "Route { level: Turn, model: \"local/qwen3:4b\", reason: Explore, explored: true, \
         propensity: 0.05, effort: Some(\"low\") }",
        "qwen3:4b (local) at low effort",
    ),
    (
        "Route { level: Subtask, model: \"claude/claude-haiku-4-5\", reason: Default, \
         explored: false, propensity: 0.95, effort: Some(\"medium\") }",
        "claude-haiku-4-5 (claude plan) at medium effort",
    ),
    (
        "Route { level: Turn, model: \"claude/claude-haiku-4-5\", reason: Default, \
         explored: false, propensity: 1.0, effort: None }",
        "claude-haiku-4-5 (claude plan) with thinking off",
    ),
    (
        "Route { level: Turn, model: \"claude/claude-opus-4-7\", reason: Escalate, \
         explored: false, propensity: 1.0, effort: Some(\"max\") }",
        "claude-opus-4-7 (claude plan) at max effort",
    ),
    (
        "Route { level: Subtask, model: \"claude/claude-sonnet-4-6\", reason: Escalate, \
         explored: false, propensity: 1.0, effort: Some(\"medium\") }",
        "claude-sonnet-4-6 (claude plan) at medium effort",
    ),
];

/// An explored pick, a kept one, a fast model with thinking off, a judged
/// failure raised to max on the strongest model and a failed subtask
/// moved up a tier, each with the level of the prompt that records it.
fn decisions() -> [(Decision, RouteLevel); 5] {
    let empty = GateStats::default();
    let settings = GateConfig::defaults();
    let debugging = easy(PromptCategory::Debugging);
    let question = easy(PromptCategory::Question);
    let explored = pick(&empty, &debugging, &claude(), &settings, &mut || 0.0);
    let kept = pick(&empty, &debugging, &claude(), &settings, &mut || 0.99);
    let off = pick(&empty, &question, &claude(), &no_exploring(), &mut || 0.0);
    let max = escalate(
        &empty,
        RouteLevel::Turn,
        (OPUS, Some(ThinkingEffort::High)),
        OutcomeSignal::JudgedFail,
        &claude(),
    );
    let up = escalate(
        &empty,
        RouteLevel::Subtask,
        (HAIKU, Some(ThinkingEffort::Off)),
        OutcomeSignal::ChecksFailed,
        &all(),
    );
    [
        (explored.unwrap(), RouteLevel::Turn),
        (kept.unwrap(), RouteLevel::Subtask),
        (off.unwrap(), RouteLevel::Turn),
        (max.unwrap(), RouteLevel::Turn),
        (up.unwrap(), RouteLevel::Subtask),
    ]
}

#[test]
fn decisions_record_their_route_and_label_byte_for_byte() {
    for ((decision, level), (route, label)) in decisions().into_iter().zip(PINNED) {
        assert_eq!(format!("{:?}", decision.route(level)), route);
        assert_eq!(decision.label(), label);
    }
}
