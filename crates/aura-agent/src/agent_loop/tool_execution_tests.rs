use aura_reasoner::ContentBlock;
use std::collections::HashMap;

use crate::constants::tool_result_cache_key;
use crate::types::ToolCallInfo;
use crate::types::ToolCallResult;

use super::tool_execution::{push_tool_result_message_with_context, split_cached};

#[test]
fn tool_results_are_emitted_before_context_texts() {
    let mut messages = Vec::new();
    let results = vec![
        ToolCallResult {
            tool_use_id: "tool_1".to_string(),
            content: "ok 1".to_string(),
            is_error: false,
            stop_loop: false,
            file_changes: Vec::new(),
        },
        ToolCallResult {
            tool_use_id: "tool_2".to_string(),
            content: "ok 2".to_string(),
            is_error: true,
            stop_loop: false,
            file_changes: Vec::new(),
        },
    ];
    let context = vec!["Build check failed".to_string()];

    push_tool_result_message_with_context(&mut messages, results, context);

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, aura_reasoner::Role::User);
    assert!(matches!(
        messages[0].content.first(),
        Some(ContentBlock::ToolResult { tool_use_id, .. }) if tool_use_id == "tool_1"
    ));
    assert!(matches!(
        messages[0].content.get(1),
        Some(ContentBlock::ToolResult { tool_use_id, .. }) if tool_use_id == "tool_2"
    ));
    assert!(matches!(
        messages[0].content.get(2),
        Some(ContentBlock::Text { text }) if text == "Build check failed"
    ));
}

#[test]
fn cached_read_hits_are_compacted_before_reinsertion() {
    let call = ToolCallInfo {
        id: "tool_1".to_string(),
        name: "read_file".to_string(),
        input: serde_json::json!({"path": "src/lib.rs"}),
    };
    let mut cache = HashMap::new();
    let long_content = "a".repeat(9_000);
    cache.insert(tool_result_cache_key(&call.name, &call.input), long_content.clone());

    let (cached, uncached) = split_cached(&[call], &cache);

    assert!(uncached.is_empty());
    assert_eq!(cached.len(), 1);
    assert!(cached[0].content.contains("Cached result reused"));
    assert!(cached[0].content.len() < long_content.len());
}
