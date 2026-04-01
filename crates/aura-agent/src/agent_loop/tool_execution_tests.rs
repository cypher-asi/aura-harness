use aura_reasoner::ContentBlock;

use crate::types::ToolCallResult;

use super::tool_execution::push_tool_result_message_with_context;

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
