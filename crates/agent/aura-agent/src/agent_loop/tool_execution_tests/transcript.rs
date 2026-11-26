use super::*;

#[test]
fn tool_results_are_emitted_before_context_texts() {
    let mut messages = Vec::new();
    let results = vec![
        ToolCallResult {
            tool_use_id: "tool_1".to_string(),
            content: "ok 1".to_string(),
            is_error: false,
            kind: aura_core_types::ToolResultKind::Ok,
            stop_loop: false,
            file_changes: Vec::new(),
            image: None,
        },
        ToolCallResult {
            tool_use_id: "tool_2".to_string(),
            content: "ok 2".to_string(),
            is_error: true,
            kind: aura_core_types::ToolResultKind::AgentError,
            stop_loop: false,
            file_changes: Vec::new(),
            image: None,
        },
    ];
    let context = vec!["Build check failed".to_string()];

    push_tool_result_message(&mut messages, results, context);

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, aura_model::Role::User);
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
fn long_tool_results_keep_their_start_and_end_in_the_transcript() {
    let result = |id: &str, content: String| ToolCallResult {
        tool_use_id: id.to_string(),
        content,
        is_error: false,
        kind: aura_core_types::ToolResultKind::Ok,
        stop_loop: false,
        file_changes: Vec::new(),
        image: None,
    };
    let short = "s".repeat(aura_config::TOOL_RESULT_MAX_BYTES);
    let long = format!("START{}END", "€".repeat(30_000));
    let mut messages = Vec::new();

    push_tool_result_message(
        &mut messages,
        vec![result("short", short.clone()), result("long", long.clone())],
        Vec::new(),
    );

    let text = |i: usize| match &messages[0].content[i] {
        ContentBlock::ToolResult { content, .. } => match content {
            aura_model::ToolResultContent::Text(t) => t.clone(),
            other => panic!("unexpected content {other:?}"),
        },
        other => panic!("unexpected block {other:?}"),
    };
    assert_eq!(text(0), short, "a result at the cap is kept whole");
    let capped = text(1);
    assert!(capped.starts_with("START") && capped.ends_with("END"));
    assert!(capped.contains("cut from the middle"));
    assert!(capped.contains(&format!("of {} bytes", long.len())));
    assert!(capped.len() < aura_config::TOOL_RESULT_MAX_BYTES + 400);
}
