use super::*;

// =========================================================================
// Integration-test equivalents (from tests/integration/full_turn.rs)
// =========================================================================

/// Equivalent of `test_simple_turn_no_tools`.
#[tokio::test]
async fn agentloop_integ_simple_no_tools() {
    let provider = MockProvider::simple_response("Hello from AURA!");
    let executor = MockExecutor { results: vec![] };
    let agent = AgentLoop::new(default_config());

    let result = agent
        .run(
            &provider,
            &executor,
            vec![Message::user("Hello, AURA!")],
            vec![],
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 1);
    assert!(result.total_text.contains("Hello from AURA!"));
    assert!(result.llm_error.is_none());
}

/// Equivalent of `test_turn_with_tool_use` (integration).
#[tokio::test]
async fn agentloop_integ_tool_use() {
    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "tool_1",
            "read_file",
            serde_json::json!({"path": "test.txt"}),
        ))
        .with_response(MockResponse::text("I read the file!"));

    let executor = MockExecutor {
        results: vec![ToolCallResult::success("placeholder", "Hello from file!")],
    };
    let agent = AgentLoop::new(default_config());

    let result = agent
        .run(
            &provider,
            &executor,
            vec![Message::user("Read test.txt")],
            vec![read_file_tool()],
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 2);

    let tool_result_count = result
        .messages
        .iter()
        .flat_map(|msg| msg.content.iter())
        .filter(|block| matches!(block, ContentBlock::ToolResult { .. }))
        .count();
    assert!(tool_result_count > 0, "tool result must be present");
}

/// Equivalent of `test_turn_tool_denied_by_policy`.
#[tokio::test]
async fn agentloop_integ_policy_deny() {
    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "tool_1",
            "dangerous_tool",
            serde_json::json!({}),
        ))
        .with_response(MockResponse::text("OK, that didn't work."));

    let executor = MockExecutor {
        results: vec![ToolCallResult::error(
            "placeholder",
            "Policy denied: tool 'dangerous_tool' is not allowed",
        )],
    };

    let agent = AgentLoop::new(default_config());

    let result = agent
        .run(
            &provider,
            &executor,
            vec![Message::user("Use dangerous tool")],
            vec![ToolDefinition::new(
                "dangerous_tool",
                "A dangerous tool",
                serde_json::json!({"type": "object"}),
            )],
        )
        .await
        .unwrap();

    let has_error_result = result.messages.iter().any(|msg| {
        msg.content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { is_error: true, .. }))
    });
    assert!(
        has_error_result,
        "denied tool calls must produce error tool_result blocks"
    );
    assert!(result.llm_error.is_none());
}

/// Equivalent of `test_turn_max_steps_limit` (integration).
///
/// See [`agentloop_max_iterations_stops`] for why `max_iterations` is N+1.
#[tokio::test]
async fn agentloop_integ_max_steps() {
    let provider = MockProvider::new().with_default_response(MockResponse::tool_use(
        "tool_1",
        "read_file",
        serde_json::json!({"path": "."}),
    ));

    let executor = MockExecutor {
        results: vec![ToolCallResult::success("placeholder", "ok")],
    };

    let config = AgentLoopConfig {
        max_iterations: 4,
        ..default_config()
    };
    let agent = AgentLoop::new(config);

    let result = agent
        .run(
            &provider,
            &executor,
            vec![Message::user("Keep running tools")],
            vec![read_file_tool()],
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 3);
}

// =========================================================================
// Integration-test equivalents (from tests/integration/multi_turn.rs)
// =========================================================================

/// Equivalent of `test_empty_history`.
#[tokio::test]
async fn agentloop_integ_empty_messages() {
    let provider = MockProvider::simple_response("Hello! Nice to meet you.");
    let executor = MockExecutor { results: vec![] };
    let agent = AgentLoop::new(default_config());

    let result = agent
        .run(
            &provider,
            &executor,
            vec![Message::user("Hi, I'm new here!")],
            vec![],
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 1);
    assert!(result.total_text.contains("Hello! Nice to meet you."));
    assert!(result.llm_error.is_none());
}
