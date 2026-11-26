use crate::model_catalog::output_limits::{CONTEXT_BOUND_OUTPUT_TOKENS, UNKNOWN_OUTPUT_TOKENS};
use crate::model_catalog::*;

#[test]
fn tiers_are_inferred_from_the_model_name() {
    assert_eq!(infer_tier("local/qwen3:4b"), ModelTier::Local);
    assert_eq!(infer_tier("claude/claude-haiku-5-5"), ModelTier::Fast);
    assert_eq!(infer_tier("chatgpt/gpt-5.4-mini"), ModelTier::Fast);
    assert_eq!(infer_tier("claude/claude-sonnet-5-5"), ModelTier::Standard);
    assert_eq!(infer_tier("claude/claude-opus-5-5"), ModelTier::Frontier);
    assert_eq!(infer_tier("chatgpt/gpt-5.6"), ModelTier::Frontier);
    assert_eq!(infer_tier("aura-gpt-5-4"), ModelTier::Frontier);
    assert_eq!(infer_tier("some-new-model"), ModelTier::Standard);
    assert!(ModelTier::Local < ModelTier::Fast && ModelTier::Standard < ModelTier::Frontier);
}

#[test]
fn replies_may_be_as_long_as_each_api_allows() {
    for (model, limit) in [
        ("aura-claude-opus-5-5", 128_000),
        ("claude/claude-sonnet-4-6", 128_000),
        ("aura-claude-haiku-4-5", 64_000),
        ("claude-sonnet-4-20250514", 64_000),
        ("claude-opus-4-1", 32_000),
        ("chatgpt/gpt-5.4", 128_000),
        ("aura-gpt-4o", 16_384),
        ("aura-gemini-3-pro", 65_536),
        ("aura-kimi-k2", CONTEXT_BOUND_OUTPUT_TOKENS),
        ("some-new-model", UNKNOWN_OUTPUT_TOKENS),
        ("local/qwen3:4b", DEFAULT_LOCAL_MAX_OUTPUT_TOKENS),
    ] {
        assert_eq!(max_output_tokens(model), limit, "{model}");
    }
}

#[test]
fn sonnet_5_5_and_opus_5_reason_high() {
    for model in [
        "claude/claude-sonnet-5-5",
        "claude-sonnet-5-5-20261001",
        "aura-claude-opus-5-5",
        "claude/claude-opus-5",
    ] {
        assert!(reasons_high(model), "{model}");
    }
    for model in [
        "claude/claude-sonnet-5",
        "claude/claude-haiku-5-5",
        "claude/claude-opus-4-8",
        "claude/claude-fable-5-2",
        "chatgpt/gpt-5.6-sol",
        "ollama/qwen3",
    ] {
        assert!(!reasons_high(model), "{model}");
    }
}

#[test]
fn max_effort_is_its_own_step_only_on_claude_models_whose_ladder_reaches_it() {
    for model in [
        "claude/claude-opus-5-5",
        "aura-claude-opus-4-8",
        "claude-sonnet-4-6",
        "claude/claude-sonnet-5-5",
        "claude/claude-fable-5-2",
    ] {
        assert!(has_max_effort(model), "{model}");
    }
    for model in [
        "claude/claude-haiku-4-5",
        "claude/claude-opus-4-1",
        "chatgpt/gpt-5.4",
        "local/qwen3:4b",
    ] {
        assert!(!has_max_effort(model), "{model}");
    }
}

#[test]
fn sonnet_and_opus_5_5_take_no_forced_tool_choice() {
    for model in [
        "claude/claude-sonnet-5-5",
        "claude-sonnet-5-5-20261001",
        "aura-claude-sonnet-5-5",
        "claude/claude-opus-5-5",
    ] {
        assert!(!takes_forced_tool_choice(model), "{model}");
    }
    for model in [
        "claude/claude-haiku-5-5",
        "claude/claude-sonnet-5",
        "claude/claude-sonnet-4-6",
        "aura-claude-opus-4-8",
        "chatgpt/gpt-5.4",
        "local/qwen3:4b",
    ] {
        assert!(takes_forced_tool_choice(model), "{model}");
    }
}

#[test]
fn a_later_version_that_costs_no_more_supersedes_the_earlier() {
    // Same family, later version, no token dearer: on any route.
    assert!(supersedes("claude-sonnet-5-5", "claude-sonnet-5"));
    assert!(supersedes(
        "claude/claude-sonnet-5-5",
        "claude/claude-sonnet-4-6"
    ));
    assert!(supersedes("claude-opus-5-5", "claude-opus-4-8"));
    assert!(supersedes("claude-opus-4-8", "claude-opus-4-7"));
    assert!(supersedes("claude-haiku-5-5", "claude-3-5-haiku"));
    assert!(supersedes("gpt-6-sol", "gpt-5.6-sol"));
    // Not the other way, nor a model itself or its dated snapshot.
    assert!(!supersedes("claude-sonnet-5", "claude-sonnet-5-5"));
    assert!(!supersedes("claude-sonnet-5-5", "claude-sonnet-5-5"));
    assert!(!supersedes("claude-haiku-4-5-20251001", "claude-haiku-4-5"));
    // A later version that costs more is a different trade.
    assert!(!supersedes("gpt-5.5", "gpt-5.4"));
    assert!(!supersedes("claude-haiku-4-5", "claude-3-5-haiku"));
    // Another family, whatever it costs.
    assert!(!supersedes("claude-haiku-5-5", "claude-sonnet-5"));
    assert!(!supersedes("gpt-5.4-mini", "gpt-5.2"));
    // Nothing is known of an unpriced or local model.
    assert!(!supersedes("claude-fable-5-2", "claude-fable-5-1"));
    assert!(!supersedes("local/qwen3:8b", "local/qwen3:4b"));
}
