use super::*;

// =========================================================================
// Unit-test equivalents (formerly in turn_processor/tests.rs)
// =========================================================================

/// Equivalent of `test_simple_turn`.
#[tokio::test]
async fn agentloop_simple_text_response() {
    let provider = MockProvider::simple_response("Hello!");
    let executor = MockExecutor { results: vec![] };
    let agent = AgentLoop::new(default_config());

    let result = agent
        .run(&provider, &executor, vec![Message::user("Hello")], vec![])
        .await
        .unwrap();

    assert_eq!(result.iterations, 1);
    assert!(result.total_text.contains("Hello!"));
}

/// Equivalent of `test_turn_with_tool_use`.
#[tokio::test]
async fn agentloop_tool_use_then_text() {
    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "tool_1",
            "read_file",
            serde_json::json!({"path": "."}),
        ))
        .with_response(MockResponse::text("I listed the files."));

    let executor = MockExecutor {
        results: vec![ToolCallResult::success(
            "placeholder",
            "file1.txt\nfile2.txt",
        )],
    };
    let agent = AgentLoop::new(default_config());

    let result = agent
        .run(
            &provider,
            &executor,
            vec![Message::user("List files")],
            vec![read_file_tool()],
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 2);
    assert!(result.total_text.contains("I listed the files."));
}

/// Text blocks from separate model calls in one turn (text, tool call,
/// text) are joined with a paragraph break instead of being glued together.
#[tokio::test]
async fn agentloop_text_segments_across_tool_calls_are_separated() {
    let provider = MockProvider::new()
        .with_response(MockResponse::text_and_tool(
            "I'll update the prompt text myself.",
            "tool_1",
            "read_file",
            serde_json::json!({"path": "."}),
        ))
        .with_response(MockResponse::text("All 3 clippy errors are fixed."));

    let executor = MockExecutor {
        results: vec![ToolCallResult::success("placeholder", "ok")],
    };
    let agent = AgentLoop::new(default_config());

    let result = agent
        .run(
            &provider,
            &executor,
            vec![Message::user("Fix clippy")],
            vec![read_file_tool()],
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 2);
    assert_eq!(
        result.total_text,
        "I'll update the prompt text myself.\n\nAll 3 clippy errors are fixed."
    );
}

/// Equivalent of `test_max_steps_limit`.
///
/// AgentLoop's budget check reserves the last iteration slot for a final
/// text response (`iteration >= max_iterations - 1`), so we set
/// `max_iterations` one higher than the desired cap to get the same
/// effective iteration count as the former TurnProcessor's `max_steps`.
#[tokio::test]
async fn agentloop_max_iterations_stops() {
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
            vec![Message::user("Keep using tools")],
            vec![read_file_tool()],
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 3);
}

/// Equivalent of `test_process_step_returns_end_turn`.
#[tokio::test]
async fn agentloop_end_turn_stops_loop() {
    let provider = MockProvider::simple_response("Done.");
    let executor = MockExecutor { results: vec![] };
    let agent = AgentLoop::new(default_config());

    let result = agent
        .run(&provider, &executor, vec![Message::user("Hello")], vec![])
        .await
        .unwrap();

    assert_eq!(result.iterations, 1);
    assert!(result.total_text.contains("Done."));
    assert!(result.llm_error.is_none());
}

/// Equivalent of `test_process_step_returns_tool_use`.
#[tokio::test]
async fn agentloop_tool_use_executes() {
    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "tool_1",
            "read_file",
            serde_json::json!({"path": "."}),
        ))
        .with_response(MockResponse::text("Read complete."));

    let executor = MockExecutor {
        results: vec![ToolCallResult::success("placeholder", "contents")],
    };
    let agent = AgentLoop::new(default_config());

    let result = agent
        .run(
            &provider,
            &executor,
            vec![Message::user("List files")],
            vec![read_file_tool()],
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 2);

    let has_tool_result = result.messages.iter().any(|msg| {
        msg.content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
    });
    assert!(has_tool_result, "tool results must be present in messages");
}
