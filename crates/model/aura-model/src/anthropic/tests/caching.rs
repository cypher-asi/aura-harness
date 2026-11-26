//! Prompt-caching markers on the system block, tools and user turns.

use crate::anthropic::api_types::ApiContent;
use crate::anthropic::convert::{
    build_system_block, convert_messages_to_api, convert_tools_to_api,
};
use crate::anthropic::{AnthropicConfig, AnthropicProvider, ReasoningOrigin};
use crate::{Message, ModelProvider, ToolDefinition};

#[test]
fn test_cache_control_on_system_block() {
    let system = build_system_block("You are a helpful assistant.", true)
        .expect("non-empty prompt emits a system block");
    let arr = system.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let block = &arr[0];
    assert_eq!(block["type"], "text");
    assert_eq!(block["text"], "You are a helpful assistant.");
    assert_eq!(block["cache_control"]["type"], "ephemeral");
}

/// Regression for `system.0: cache_control cannot be set for empty text
/// blocks`. Chat sessions enter `complete()` with `system = ""` (see
/// `crates/surface/aura-node/src/session/state.rs` `Session::new`), so the
/// build helper must omit the block entirely instead of producing an
/// empty `text` block decorated with `cache_control`.
#[test]
fn build_system_block_returns_none_for_empty_prompt_even_with_caching() {
    assert!(build_system_block("", true).is_none());
    assert!(build_system_block("", false).is_none());
    assert!(build_system_block("   \n\t  ", true).is_none());
}

#[test]
fn test_cache_control_on_last_tool() {
    let tools = vec![
        ToolDefinition::new(
            "fs.read",
            "Read a file",
            serde_json::json!({"type": "object"}),
        ),
        ToolDefinition::new(
            "fs.write",
            "Write a file",
            serde_json::json!({"type": "object"}),
        ),
    ];

    let api_tools = convert_tools_to_api(&tools, true);
    assert_eq!(api_tools.len(), 2);
    assert!(api_tools[0].cache_control.is_none());
    let last_cc = api_tools[1].cache_control.as_ref().unwrap();
    assert_eq!(last_cc["type"], "ephemeral");
}

#[test]
fn test_cache_control_on_last_user_message() {
    let messages = vec![
        Message::user("Hello"),
        Message::assistant("Hi!"),
        Message::user("How are you?"),
    ];

    let api_msgs = convert_messages_to_api(&messages, true, ReasoningOrigin::Anthropic);

    let last_user = &api_msgs[2];
    assert_eq!(last_user.role, "user");
    if let ApiContent::Text { cache_control, .. } = &last_user.content[0] {
        let cc = cache_control.as_ref().unwrap();
        assert_eq!(cc["type"], "ephemeral");
    } else {
        panic!("Expected Text content");
    }
}

#[test]
fn test_cache_control_on_last_two_user_messages_only() {
    let messages = vec![
        Message::user("first"),
        Message::assistant("a"),
        Message::user("second"),
        Message::assistant("b"),
        Message::user("third"),
    ];

    let api_msgs = convert_messages_to_api(&messages, true, ReasoningOrigin::Anthropic);
    let marked: Vec<bool> = api_msgs
        .iter()
        .map(|m| {
            m.content.iter().any(|block| {
                matches!(
                    block,
                    ApiContent::Text {
                        cache_control: Some(_),
                        ..
                    }
                )
            })
        })
        .collect();

    assert_eq!(marked, vec![false, false, true, false, true]);
}

#[test]
fn test_beta_header_present() {
    let config = AnthropicConfig::new("test-model");
    let provider = AnthropicProvider::new(config).unwrap();

    let system = build_system_block("test", true).expect("non-empty prompt emits a system block");
    let json = serde_json::to_string(&system).unwrap();
    assert!(json.contains("cache_control"));
    assert!(json.contains("ephemeral"));

    assert_eq!(provider.name(), "anthropic");
}

#[test]
fn test_cache_control_omitted_when_prompt_caching_disabled() {
    let system = build_system_block("test", false).expect("non-empty prompt emits a system block");
    let json = serde_json::to_string(&system).unwrap();
    assert!(!json.contains("cache_control"));

    let messages = vec![
        Message::user("Hello"),
        Message::assistant("Hi!"),
        Message::user("How are you?"),
    ];
    let api_msgs = convert_messages_to_api(&messages, false, ReasoningOrigin::Anthropic);
    if let ApiContent::Text { cache_control, .. } = &api_msgs[2].content[0] {
        assert!(cache_control.is_none());
    } else {
        panic!("Expected Text content");
    }

    let tools = vec![
        ToolDefinition::new(
            "fs.read",
            "Read a file",
            serde_json::json!({"type": "object"}),
        ),
        ToolDefinition::new(
            "fs.write",
            "Write a file",
            serde_json::json!({"type": "object"}),
        ),
    ];
    let api_tools = convert_tools_to_api(&tools, false);
    assert!(api_tools.iter().all(|tool| tool.cache_control.is_none()));
}

#[test]
fn test_anthropic_request_caches_last_tool_when_no_tool_cache_control_set() {
    let tools = vec![
        ToolDefinition::new(
            "fs.read",
            "Read a file",
            serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}}
            }),
        ),
        ToolDefinition::new(
            "fs.write",
            "Write a file",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "contents": {"type": "string"}
                }
            }),
        ),
    ];

    let api_tools = convert_tools_to_api(&tools, true);
    assert_eq!(api_tools.len(), 2);
    assert!(
        api_tools[0].cache_control.is_none(),
        "First tool should not be cached when no explicit cache_control is set"
    );
    assert_eq!(
        api_tools[1].cache_control,
        Some(serde_json::json!({"type": "ephemeral"})),
        "Last tool should default to ephemeral cache_control when prompt caching is enabled"
    );
}
