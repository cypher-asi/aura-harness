use super::support::{assert_cost, options_of, usage};
use crate::model_catalog::*;

#[test]
fn cost_is_priced_the_same_on_every_route() {
    let usage = usage(1_000, 2_000, 1_000_000, 1_000);
    for model in [
        "claude-opus-4-7",
        "claude/claude-opus-4-7",
        "aura-claude-opus-4-7",
    ] {
        assert_cost(model, &usage, 0.5425);
        assert_eq!(call_tokens(model, &usage), 1_004_000);
    }
}

#[test]
fn openai_cached_tokens_are_part_of_the_input() {
    let usage = usage(10_000, 0, 8_000, 1_000);
    for model in ["gpt-5.4", "chatgpt/gpt-5.4", "aura-gpt-5-4"] {
        assert_cost(model, &usage, 0.022);
        assert_eq!(call_tokens(model, &usage), 11_000);
    }
}

#[test]
fn dated_snapshots_share_their_model_price() {
    let usage = usage(1_000_000, 0, 0, 0);
    assert_cost("claude-haiku-4-5-20251001", &usage, 1.0);
    assert_cost("claude-opus-4-20250514", &usage, 15.0);
    assert_cost("gpt-5.4-2026-03-05", &usage, 2.5);
    assert_cost("claude-opus-5-5", &usage, 4.0);
}

#[test]
fn local_and_unknown_models_have_no_cost() {
    let usage = usage(1_000, 0, 0, 1_000);
    assert_eq!(estimate_cost_usd("local/qwen3:4b", &usage), None);
    assert_eq!(estimate_cost_usd("qwen3:4b", &usage), None);
    assert_eq!(estimate_cost_usd("gpt-5-3", &usage), None);
    assert_eq!(estimate_cost_usd("claude-opus-9", &usage), None);
    assert_eq!(call_tokens("qwen3:4b", &usage), 2_000);
}

#[test]
fn openrouter_models_share_curated_tiers_limits_and_prices() {
    assert_eq!(
        infer_tier("openrouter/anthropic/claude-haiku-4.5"),
        ModelTier::Fast
    );
    assert_eq!(
        infer_tier("openrouter/openai/gpt-5.4:nitro"),
        ModelTier::Frontier
    );
    assert_eq!(
        max_output_tokens("openrouter/anthropic/claude-haiku-4.5"),
        64_000
    );
    let usage = usage(1_000, 2_000, 1_000_000, 1_000);
    let direct = estimate_cost_usd("claude/claude-sonnet-4-6", &usage);
    assert!(direct.is_some());
    assert_eq!(
        estimate_cost_usd("openrouter/anthropic/claude-sonnet-4.6", &usage),
        direct
    );
    assert!(options_of(ModelBackend::OpenRouter)
        .contains(&"openrouter/anthropic/claude-sonnet-4.6".to_string()));
}

#[test]
fn openrouter_usage_is_anthropic_shaped_even_for_openai_models() {
    let usage = usage(10_000, 0, 8_000, 1_000);
    assert_eq!(call_tokens("openrouter/openai/gpt-5.4", &usage), 19_000);
    assert_eq!(call_tokens("chatgpt/gpt-5.4", &usage), 11_000);
}

#[test]
fn a_bare_vendor_slug_is_not_taken_for_an_openrouter_id() {
    // Local servers name models the way OpenRouter does
    // (`openai/gpt-oss-20b` on LM Studio), so a name with no route
    // prefix is not priced, keyed or shaped as OpenRouter's.
    let slug = "openai/gpt-6.1-sol";
    assert_eq!(backend_for(slug), ModelBackend::Router);
    assert_eq!(model_key(slug), "openai/gpt-6-1-sol");
    assert_ne!(model_key(slug), model_key("openrouter/openai/gpt-6.1-sol"));
    let usage = usage(10_000, 0, 8_000, 1_000);
    assert_eq!(estimate_cost_usd(slug, &usage), None);
    assert!(estimate_cost_usd("openrouter/openai/gpt-6.1-sol", &usage).is_some());

    // Router aliases and bare GPT names keep their OpenAI shape.
    assert_eq!(model_key("aura-gpt-5-4"), "gpt-5-4");
    assert_eq!(call_tokens("aura-gpt-5-4", &usage), 11_000);
    assert_eq!(call_tokens("gpt-6.1-sol", &usage), 11_000);
}
