//! `resolve_thinking` defaults per model family.

use super::support::*;
use crate::anthropic::convert::resolve_thinking;
use crate::{ModelRequest, ThinkingConfig};

#[test]
fn test_resolve_thinking_explicit_config() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(8192)
        .thinking(ThinkingConfig {
            budget_tokens: 4000,
        })
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, TEST_DEFAULT_MODEL);
    assert!(thinking.is_some());
    let thinking = thinking.unwrap();
    assert_eq!(thinking.thinking_type, "adaptive");
    assert_eq!(thinking.budget_tokens, None);
}

#[test]
fn test_resolve_thinking_auto_for_capable_model() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(8192)
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, TEST_DEFAULT_MODEL);
    assert!(thinking.is_some());
    let thinking = thinking.unwrap();
    assert_eq!(thinking.thinking_type, "adaptive");
    assert_eq!(thinking.budget_tokens, None);
}

#[test]
fn test_resolve_thinking_auto_for_aura_alias_capable_model() {
    let request = ModelRequest::builder("aura-claude-opus-4-7", "system")
        .max_tokens(8192)
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, "aura-claude-opus-4-7").unwrap();
    assert_eq!(thinking.thinking_type, "adaptive");
    assert_eq!(thinking.budget_tokens, None);
}

#[test]
fn test_resolve_thinking_requests_summarized_display() {
    // Opus 4.7+ / Claude 5 default `display` to "omitted" (empty thinking
    // text); we must opt into summaries so reasoning is visible.
    for model in [
        "aura-claude-opus-4-7",
        "aura-claude-opus-5",
        "claude-3-7-sonnet",
    ] {
        let request = ModelRequest::builder(model, "system")
            .max_tokens(8192)
            .try_build()
            .unwrap();
        let thinking = resolve_thinking(&request, model).unwrap();
        assert_eq!(thinking.display, Some("summarized"), "{model}");
        let json = serde_json::to_value(&thinking).unwrap();
        assert_eq!(json["display"], "summarized", "{model}");
    }
}

#[test]
fn test_resolve_thinking_auto_for_opus_5() {
    let request = ModelRequest::builder("aura-claude-opus-5", "system")
        .max_tokens(8192)
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, "aura-claude-opus-5").unwrap();
    assert_eq!(thinking.thinking_type, "adaptive");
    assert_eq!(thinking.budget_tokens, None);
}

#[test]
fn test_resolve_thinking_auto_for_fable_and_mythos_5_1() {
    for model in ["aura-claude-fable-5-1", "aura-claude-mythos-5-1"] {
        let request = ModelRequest::builder(model, "system")
            .max_tokens(128_000)
            .try_build()
            .unwrap();
        let thinking = resolve_thinking(&request, model).expect("5.1 thinking config");
        assert_eq!(thinking.thinking_type, "adaptive", "{model}");
        assert_eq!(thinking.budget_tokens, None, "{model}");
    }
}

#[test]
fn test_resolve_thinking_uses_enabled_budget_for_older_models() {
    let request = ModelRequest::builder("claude-3-7-sonnet", "system")
        .max_tokens(8192)
        .thinking(ThinkingConfig {
            budget_tokens: 4000,
        })
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, "claude-3-7-sonnet").unwrap();
    assert_eq!(thinking.thinking_type, "enabled");
    assert_eq!(thinking.budget_tokens, Some(4000));
}

#[test]
fn test_resolve_thinking_none_for_unsupported_haiku_variants() {
    let request = ModelRequest::builder("aura-claude-haiku-4-5", "system")
        .max_tokens(8192)
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, "aura-claude-haiku-4-5");
    assert!(thinking.is_none());
}

#[test]
fn test_resolve_thinking_auto_for_non_claude_model_is_none() {
    let request = ModelRequest::builder("aura-gpt-5-4", "system")
        .max_tokens(8192)
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, "aura-gpt-5-4");
    assert!(thinking.is_none());
}

#[test]
fn test_resolve_thinking_none_for_small_budget() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(1024)
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, TEST_DEFAULT_MODEL);
    assert!(thinking.is_none());
}

#[test]
fn test_resolve_thinking_none_for_unsupported_claude_3_variants() {
    let request = ModelRequest::builder("claude-3-haiku", "system")
        .max_tokens(8192)
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, "claude-3-haiku");
    assert!(thinking.is_none());
}
