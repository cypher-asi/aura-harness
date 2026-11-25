//! Which model classifies.

use aura_core_auth::ProviderKind;
use aura_gate_route::{candidates, Candidate};
use aura_model::model_catalog::{catalog, ModelBackend, ModelTier};

use super::classifier;

#[test]
fn the_cheapest_fast_model_classifies() {
    let options = catalog("local/qwen3:4b", &["qwen3:4b".to_string()]);
    let all = candidates(
        &options,
        |p| matches!(p, ProviderKind::Zos | ProviderKind::Claude),
        |_| false,
    );
    assert_eq!(
        classifier(&all).map(|c| c.id.as_str()),
        Some("claude/claude-haiku-4-5")
    );
    let local_only = candidates(&options, |_| false, |_| false);
    assert_eq!(classifier(&local_only), None);
}

#[test]
fn a_fast_model_on_a_linked_plan_classifies_before_a_cheaper_paid_one() {
    let candidate = |id: &str, backend, tier, cost| Candidate {
        id: id.to_string(),
        label: id.to_string(),
        backend,
        tier,
        cost_usd: Some(cost),
    };
    let nano = candidate(
        "openrouter/openai/gpt-5-nano",
        ModelBackend::OpenRouter,
        ModelTier::Fast,
        0.001,
    );
    let haiku = candidate(
        "claude/claude-haiku-4-5",
        ModelBackend::Claude,
        ModelTier::Fast,
        0.02,
    );
    let sonnet = candidate(
        "claude/claude-sonnet-4-6",
        ModelBackend::Claude,
        ModelTier::Standard,
        0.06,
    );
    let all = [nano.clone(), haiku, sonnet.clone()];
    assert_eq!(
        classifier(&all).map(|c| c.id.as_str()),
        Some("claude/claude-haiku-4-5")
    );
    // With no fast model on a plan, the cheapest fast one.
    assert_eq!(
        classifier(&[nano, sonnet]).map(|c| c.id.as_str()),
        Some("openrouter/openai/gpt-5-nano")
    );
}
