//! Message, tool and tool-choice conversion, model chains, capability
//! slugs and response parsing.

use super::support::*;
use crate::anthropic::api_types::ApiToolChoice;
use crate::anthropic::convert::{
    capability_model, convert_messages_to_api, convert_tool_choice, convert_tools_to_api,
    resolve_output_config, resolve_thinking, tool_choice_for,
};
use crate::anthropic::{AnthropicConfig, AnthropicProvider, ReasoningOrigin};
use crate::{ContentBlock, Message, ModelRequest, ThinkingEffort, ToolChoice, ToolDefinition};

#[test]
fn test_config_new() {
    let config = AnthropicConfig::new("claude-3-haiku");
    assert_eq!(config.default_model, "claude-3-haiku");
    assert_eq!(config.base_url, "https://aura-router.onrender.com");
    assert!(config.prompt_caching_enabled);
}

#[test]
fn test_convert_messages() {
    let messages = vec![Message::user("Hello"), Message::assistant("Hi there!")];

    let api_msgs = convert_messages_to_api(&messages, true, ReasoningOrigin::Anthropic);
    assert_eq!(api_msgs.len(), 2);
    assert_eq!(api_msgs[0].role, "user");
    assert_eq!(api_msgs[1].role, "assistant");
}

#[test]
fn test_convert_tools() {
    let tools = vec![ToolDefinition::new(
        "fs.read",
        "Read a file",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            }
        }),
    )];

    let api_tools = convert_tools_to_api(&tools, true);
    assert_eq!(api_tools.len(), 1);
    assert_eq!(api_tools[0].name, "fs.read");
}

#[test]
fn test_convert_tool_choice() {
    assert!(matches!(
        convert_tool_choice(&ToolChoice::Auto, true),
        Some(ApiToolChoice::Auto {
            disable_parallel_tool_use: None
        })
    ));
    assert!(matches!(
        convert_tool_choice(&ToolChoice::Required, true),
        Some(ApiToolChoice::Any {
            disable_parallel_tool_use: None
        })
    ));
    assert!(convert_tool_choice(&ToolChoice::None, true).is_none());
}

// ---------------------------------------------------------------------------
// Phase 3 — parallel tool use wire shape
// ---------------------------------------------------------------------------

#[test]
fn parallel_tool_use_default_true_omits_disable_field() {
    // Default `parallel_tool_use: true` must NOT emit
    // `disable_parallel_tool_use`. Confirms the wire payload stays
    // byte-identical to the pre-Phase-3 `{"type": "auto"}` shape, so
    // the existing log-summary / golden-body tests keep passing and
    // we don't accidentally regress on the wire when nothing
    // changes.
    let choice = convert_tool_choice(&ToolChoice::Auto, true).expect("Auto -> Some");
    let json = serde_json::to_value(&choice).unwrap();
    assert_eq!(json, serde_json::json!({ "type": "auto" }));
}

#[test]
fn parallel_tool_use_false_emits_disable_parallel_tool_use_true() {
    let choice = convert_tool_choice(&ToolChoice::Auto, false).expect("Auto -> Some");
    let json = serde_json::to_value(&choice).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "type": "auto", "disable_parallel_tool_use": true })
    );
}

#[test]
fn parallel_tool_use_false_propagates_to_required_and_tool_variants() {
    let any_choice = convert_tool_choice(&ToolChoice::Required, false).expect("Required -> Some");
    let any_json = serde_json::to_value(&any_choice).unwrap();
    assert_eq!(
        any_json,
        serde_json::json!({ "type": "any", "disable_parallel_tool_use": true })
    );

    let tool_choice = convert_tool_choice(
        &ToolChoice::Tool {
            name: "read_file".to_string(),
        },
        false,
    )
    .expect("Tool -> Some");
    let tool_json = serde_json::to_value(&tool_choice).unwrap();
    assert_eq!(
        tool_json,
        serde_json::json!({
            "type": "tool",
            "name": "read_file",
            "disable_parallel_tool_use": true,
        })
    );
}

#[test]
fn model_request_default_parallel_tool_use_is_true() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(8_192)
        .try_build()
        .unwrap();
    assert!(
        request.parallel_tool_use,
        "default must be true so codex-style batching is on by default"
    );
}

#[test]
fn model_request_parallel_tool_use_builder_override() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(8_192)
        .parallel_tool_use(false)
        .try_build()
        .unwrap();
    assert!(
        !request.parallel_tool_use,
        "builder override must propagate to ModelRequest.parallel_tool_use"
    );
}

#[test]
fn test_config_with_fallback() {
    let mut config = AnthropicConfig::new(TEST_DEFAULT_MODEL);
    config.fallback_model = Some(TEST_FALLBACK_MODEL.to_string());
    assert_eq!(config.fallback_model, Some(TEST_FALLBACK_MODEL.to_string()));
}

#[test]
fn test_model_chain_without_fallback() {
    let config = AnthropicConfig::new(TEST_DEFAULT_MODEL);
    let provider = AnthropicProvider::new(config).unwrap();
    let chain = provider.model_chain(TEST_DEFAULT_MODEL);
    assert_eq!(chain, vec![TEST_DEFAULT_MODEL]);
}

#[test]
fn test_model_chain_with_fallback() {
    let mut config = AnthropicConfig::new(TEST_DEFAULT_MODEL);
    config.fallback_model = Some(TEST_FALLBACK_MODEL.to_string());
    let provider = AnthropicProvider::new(config).unwrap();
    let chain = provider.model_chain(TEST_DEFAULT_MODEL);
    assert_eq!(chain, vec![TEST_DEFAULT_MODEL, TEST_FALLBACK_MODEL]);
}

#[test]
fn test_model_chain_deduplicates() {
    let mut config = AnthropicConfig::new(TEST_DEFAULT_MODEL);
    config.fallback_model = Some(TEST_DEFAULT_MODEL.to_string());
    let provider = AnthropicProvider::new(config).unwrap();
    let chain = provider.model_chain(TEST_DEFAULT_MODEL);
    assert_eq!(chain, vec![TEST_DEFAULT_MODEL]);
}

/// OpenRouter names models `vendor/model` (with dots, and maybe a
/// `:variant`); thinking, effort and forced tool choice follow the model
/// it names, as they do for the same model on any other route.
#[test]
fn openrouter_slugs_get_the_capabilities_of_the_model_they_name() {
    assert_eq!(
        capability_model("anthropic/claude-sonnet-4.6:nitro"),
        "claude-sonnet-4.6"
    );
    assert_eq!(capability_model("claude-opus-4-7"), "claude-opus-4-7");
    assert_eq!(capability_model("anthropic/"), "anthropic/");

    let request = |model: &str| {
        ModelRequest::builder(model, "system")
            .max_tokens(8192)
            .thinking_effort(Some(ThinkingEffort::High))
            .try_build()
            .unwrap()
    };
    for (slug, direct) in [
        ("anthropic/claude-opus-4.7", "claude-opus-4-7"),
        ("anthropic/claude-sonnet-4.6", "claude-sonnet-4-6"),
    ] {
        let via_openrouter = resolve_thinking(&request(slug), slug).expect(slug);
        let direct_thinking = resolve_thinking(&request(direct), direct).unwrap();
        assert_eq!(
            via_openrouter.thinking_type, direct_thinking.thinking_type,
            "{slug}"
        );
        assert_eq!(
            resolve_output_config(&request(slug), slug).map(|c| c.effort),
            resolve_output_config(&request(direct), direct).map(|c| c.effort),
            "{slug}"
        );
    }

    let forced = ToolChoice::Required;
    assert!(
        matches!(
            tool_choice_for(&forced, false, "anthropic/claude-sonnet-5.5"),
            Some(ApiToolChoice::Auto { .. })
        ),
        "Sonnet 5.5 takes no forced tool choice on OpenRouter either"
    );
    assert!(matches!(
        tool_choice_for(&forced, false, "anthropic/claude-sonnet-4.6"),
        Some(ApiToolChoice::Any { .. })
    ));
}

#[test]
fn complete_response_with_redacted_thinking_parses_and_drops_it() {
    // The reply Gate's classifier got from `openrouter/openai/gpt-5-nano`
    // through OpenRouter's Anthropic endpoint: encrypted reasoning, then
    // the forced `label` call. It failed to parse before.
    let body = r#"{
        "id": "gen-1",
        "type": "message",
        "role": "assistant",
        "model": "openai/gpt-5-nano",
        "content": [
            {"type": "redacted_thinking", "data": "gAAAAABencrypted"},
            {"type": "tool_use", "id": "call_1", "name": "label",
             "input": {"category": "code", "difficulty": "medium"}}
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 812, "output_tokens": 64}
    }"#;
    let response: crate::anthropic::api_types::ApiResponse = serde_json::from_str(body).unwrap();
    assert_eq!(response.usage.input_tokens, 812);
    let message = crate::anthropic::convert::convert_response_to_aura(&response.content);
    assert_eq!(message.content.len(), 1);
    assert!(matches!(
        &message.content[0],
        ContentBlock::ToolUse { name, .. } if name == "label"
    ));
}

#[test]
fn complete_response_with_an_unknown_block_type_keeps_the_rest() {
    let body = r#"{
        "id": "msg_1",
        "model": "claude-x",
        "content": [
            {"type": "web_search_tool_result", "tool_use_id": "srv_1", "content": []},
            {"type": "text", "text": "done"}
        ],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 1, "output_tokens": 1}
    }"#;
    let response: crate::anthropic::api_types::ApiResponse = serde_json::from_str(body).unwrap();
    let message = crate::anthropic::convert::convert_response_to_aura(&response.content);
    assert!(matches!(
        message.content.as_slice(),
        [ContentBlock::Text { text }] if text == "done"
    ));
}
