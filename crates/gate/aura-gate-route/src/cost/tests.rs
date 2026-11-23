//! How observed costs order the candidates and price the untried ones.

use aura_core_auth::ProviderKind;
use aura_core_types::{PromptCategory, RouteLevel};
use aura_model::model_catalog::{catalog_with_plans, ModelBackend, ModelOption, ModelTier};

use super::by_expected_cost;
use crate::fixtures::{chosen, claude, priced_turn, signed_in, HAIKU, LOCAL, OPUS, SONNET};
use crate::stats::GateStats;
use crate::{candidates, Candidate, Work};

#[test]
fn observed_costs_reorder_the_candidates_and_scale_the_rest() {
    let claude = claude();
    let empty = GateStats::default();
    let ids = |order: Vec<&Candidate>| -> Vec<String> {
        order.into_iter().map(|c| c.id.clone()).collect()
    };
    assert_eq!(
        ids(by_expected_cost(&empty, RouteLevel::Turn, &claude)),
        [LOCAL, HAIKU, SONNET, OPUS]
    );

    let list = |id: &str| {
        claude
            .iter()
            .find(|c| c.id == id)
            .unwrap()
            .cost_usd
            .unwrap()
    };
    let question = Work::turn(PromptCategory::Question);
    let mut stats = GateStats::default();
    priced_turn(&mut stats, &question, HAIKU, true, 1_000.0);
    priced_turn(&mut stats, &question, SONNET, true, list(SONNET));
    let order = ids(by_expected_cost(&stats, RouteLevel::Turn, &claude));
    let at = |id: &str| order.iter().position(|o| o == id).unwrap();
    assert_eq!(order[0], LOCAL);
    assert!(at(SONNET) < at(HAIKU), "{order:?}");

    let mut stats = GateStats::default();
    priced_turn(&mut stats, &question, SONNET, true, list(SONNET) * 10.0);
    assert_eq!(
        ids(by_expected_cost(&stats, RouteLevel::Turn, &claude)),
        [LOCAL, HAIKU, SONNET, OPUS],
        "turns costing 10x list price make the untried models 10x too"
    );
    assert!(by_expected_cost(&stats, RouteLevel::Subtask, &claude)
        .iter()
        .map(|c| c.id.as_str())
        .eq([LOCAL, HAIKU, SONNET, OPUS]));
}

const SONNET_5: &str = "claude/claude-sonnet-5-5";
const OPUS_5: &str = "claude/claude-opus-5-5";

/// The Claude plan with Haiku 4.5 and two versions each of Sonnet and
/// Opus.
fn two_of_each() -> Vec<Candidate> {
    let live: Vec<ModelOption> = [
        (HAIKU, ModelTier::Fast),
        (SONNET, ModelTier::Standard),
        (SONNET_5, ModelTier::Standard),
        (OPUS, ModelTier::Frontier),
        (OPUS_5, ModelTier::Frontier),
    ]
    .into_iter()
    .map(|(id, tier)| ModelOption {
        id: id.to_string(),
        label: id.to_string(),
        backend: ModelBackend::Claude,
        installed: true,
        tier,
    })
    .collect();
    let options = catalog_with_plans(LOCAL, &[], &live);
    candidates(&options, signed_in(&[ProviderKind::Claude]), |_| false)
}

#[test]
fn an_untried_model_is_priced_from_the_tried_models_of_its_tier() {
    let plan = two_of_each();
    // Sonnet 4.6 took the light work and Opus 4.7 the heavy, so
    // Sonnet's turns cost far less against list price than Opus's.
    let mut stats = GateStats::default();
    let small_change = Work::turn(PromptCategory::SmallChange);
    priced_turn(&mut stats, &small_change, SONNET, true, 0.02);
    let debugging = Work::turn(PromptCategory::Debugging);
    priced_turn(&mut stats, &debugging, OPUS, true, 0.60);
    let order: Vec<&str> = by_expected_cost(&stats, RouteLevel::Turn, &plan)
        .into_iter()
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(
        order,
        [HAIKU, SONNET_5, SONNET, OPUS_5, OPUS],
        "the untried fast model is priced from the standard tier's turns"
    );
    assert_eq!(
        chosen(&stats, PromptCategory::Question, &plan).0,
        SONNET_5,
        "the cheaper standard model stays the default"
    );
    assert_eq!(chosen(&stats, PromptCategory::Debugging, &plan).0, SONNET_5);
    assert_eq!(chosen(&stats, PromptCategory::Planning, &plan).0, SONNET_5);
}
