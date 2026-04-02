use aura_reasoner::ContentBlock;
use aura_reasoner::Message;
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

#[test]
fn repeated_cached_reads_reduce_message_footprint_across_turns() {
    let call = ToolCallInfo {
        id: "tool_1".to_string(),
        name: "read_file".to_string(),
        input: serde_json::json!({"path": "src/lib.rs"}),
    };
    let mut cache = HashMap::new();
    let long_content = "a".repeat(9_000);
    cache.insert(tool_result_cache_key(&call.name, &call.input), long_content.clone());

    let mut shaped_messages = vec![Message::user("Read the same file again.")];
    let (shaped_cached, _) = split_cached(std::slice::from_ref(&call), &cache);
    push_tool_result_message_with_context(&mut shaped_messages, shaped_cached, Vec::new());

    let mut unshaped_messages = vec![Message::user("Read the same file again.")];
    push_tool_result_message_with_context(
        &mut unshaped_messages,
        vec![ToolCallResult::success("tool_1", &long_content)],
        Vec::new(),
    );

    let shaped_chars = crate::compaction::estimate_message_chars(&shaped_messages);
    let unshaped_chars = crate::compaction::estimate_message_chars(&unshaped_messages);
    let saved_chars = unshaped_chars.saturating_sub(shaped_chars);

    assert!(shaped_chars < unshaped_chars);
    assert!(
        saved_chars >= 4_500,
        "expected at least 4.5k chars saved across repeated turn, got {saved_chars}"
    );
}
