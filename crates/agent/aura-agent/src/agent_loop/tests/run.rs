use super::*;

#[test]
fn test_agent_loop_config_defaults() {
    let config = AgentLoopConfig::for_agent("claude-test-model");
    // Default flows from `aura_core_types::MAX_TURNS` — the single source of
    // truth shared with the runtime session, agent runner, subagent
    // budgets, and integration-test harness. Termination is still
    // driven by `EndTurn`, the credit budget, or cooperative
    // cancellation; this assertion just pins the per-turn cap and the
    // companion E.1 caps to the same canonical value.
    assert_eq!(config.max_iterations, aura_core_types::MAX_TURNS as usize);
    assert_eq!(config.max_turns_per_task, aura_core_types::MAX_TURNS);
    assert_eq!(config.max_iterations_per_task, aura_core_types::MAX_TURNS);
    assert_eq!(config.auto_build_cooldown, 2);
    assert_eq!(config.thinking_taper_after, 2);
    assert!((config.thinking_taper_factor - 0.6).abs() < f64::EPSILON);
    // Floor raised from 1024 → 6144 to fit a full-size tool-call JSON
    // (harness observed `edit_file` truncations at ~2.5 KB / ~1000
    // tokens plus preceding reasoning). See `constants::THINKING_MIN_BUDGET`.
    assert_eq!(config.thinking_min_budget, 6144);
    // The default config carries no exploration-reset signal; only
    // `execute_task_tracked` wires one through.
    assert!(config.phase_reset_signal.is_none());
}

#[tokio::test]
async fn test_agent_loop_simple_run() {
    let config = AgentLoopConfig::for_agent("claude-test-model");
    let agent = AgentLoop::new(config);
    let executor = MockExecutor { results: vec![] };
    let provider = MockProvider::simple_response("Hello!");
    let messages = vec![Message::user("hello")];
    let tools = vec![];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();
    assert_eq!(result.iterations, 1);
    assert!(result.total_text.contains("Hello!"));
    assert!(result.total_input_tokens > 0);
}

#[tokio::test]
async fn test_agent_loop_full_integration() {
    let executor = MockExecutor {
        results: vec![ToolCallResult::success("placeholder", "file contents here")],
    };

    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "tool_1",
            "read_file",
            serde_json::json!({"path": "test.txt"}),
        ))
        .with_response(MockResponse::text("All done!"));

    let config = AgentLoopConfig {
        system_prompt: "You are a test agent".to_string(),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let agent = AgentLoop::new(config);
    let messages = vec![Message::user("Read test.txt")];
    let tools = vec![ToolDefinition::new(
        "read_file",
        "Read a file",
        serde_json::json!({"type": "object"}),
    )];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(result.iterations, 2);
    assert!(result.total_text.contains("All done!"));
    assert!(result.total_input_tokens > 0);
    assert!(result.total_output_tokens > 0);
    assert!(!result.insufficient_credits);
    assert!(result.llm_error.is_none());
}

#[tokio::test]
async fn final_text_is_the_last_reply_without_remarks_between_tools() {
    let executor = MockExecutor {
        results: vec![ToolCallResult::success("placeholder", "file contents here")],
    };
    let provider = MockProvider::new()
        .with_response(MockResponse::text_and_tool(
            "Let me read the file first.",
            "tool_1",
            "read_file",
            serde_json::json!({"path": "test.txt"}),
        ))
        .with_response(MockResponse::text("The file says hello."));

    let agent = AgentLoop::new(AgentLoopConfig::for_agent("claude-test-model"));
    let tools = vec![ToolDefinition::new(
        "read_file",
        "Read a file",
        serde_json::json!({"type": "object"}),
    )];
    let result = agent
        .run(
            &provider,
            &executor,
            vec![Message::user("Read test.txt")],
            tools,
        )
        .await
        .unwrap();

    assert!(result.total_text.contains("Let me read the file first."));
    assert_eq!(result.final_text, "The file says hello.");
}

#[tokio::test]
async fn test_agent_loop_402_insufficient_credits() {
    let executor = MockExecutor { results: vec![] };
    let provider = MockProvider::new().with_failure();

    let config = AgentLoopConfig::for_agent("claude-test-model");
    let agent = AgentLoop::new(config);
    let messages = vec![Message::user("hello")];
    let tools = vec![];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();
    assert!(result.llm_error.is_some());
}

#[tokio::test]
async fn test_max_tokens_with_pending_tools_injects_errors() {
    let executor = MockExecutor { results: vec![] };

    let provider = MockProvider::new()
        .with_response(
            MockResponse::tool_use(
                "tool_1",
                "read_file",
                serde_json::json!({"path": "big_file.txt"}),
            )
            .with_stop_reason(StopReason::MaxTokens),
        )
        .with_response(MockResponse::text("Recovered after truncation."));

    let config = AgentLoopConfig {
        system_prompt: "Test agent".to_string(),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let agent = AgentLoop::new(config);
    let messages = vec![Message::user("Read big_file.txt")];
    let tools = vec![ToolDefinition::new(
        "read_file",
        "Read a file",
        serde_json::json!({"type": "object"}),
    )];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(
        result.iterations, 2,
        "Loop should continue after MaxTokens with pending tools"
    );
    assert!(result.total_text.contains("Recovered after truncation."));

    let has_error_tool_result = result.messages.iter().any(|msg| {
        msg.content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { is_error: true, .. }))
    });
    assert!(
        has_error_tool_result,
        "Should have injected an error tool result"
    );
}

#[tokio::test]
async fn test_max_tokens_without_tools_breaks() {
    let executor = MockExecutor { results: vec![] };

    let provider = MockProvider::new()
        .with_response(MockResponse::text("Truncated text").with_stop_reason(StopReason::MaxTokens))
        .with_response(MockResponse::text("Should not reach this"));

    let config = AgentLoopConfig {
        system_prompt: "Test agent".to_string(),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let agent = AgentLoop::new(config);
    let messages = vec![Message::user("hello")];
    let tools = vec![];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(
        result.iterations, 2,
        "Loop should continue a truncated text response"
    );
    assert!(result.total_text.contains("Truncated text"));
    assert!(result.total_text.contains("Should not reach this"));
}

#[tokio::test]
async fn responses_cut_off_before_acting_are_told_so_and_only_in_a_row_end_the_run() {
    let executor = MockExecutor {
        results: vec![ToolCallResult::success("tool_1", "ok")],
    };
    let cut =
        || MockResponse::text("Let me write the module.").with_stop_reason(StopReason::MaxTokens);
    let provider = MockProvider::new()
        .with_response(cut())
        .with_response(cut())
        .with_response(MockResponse::tool_use(
            "tool_1",
            "read_file",
            serde_json::json!({"path": "a.py"}),
        ))
        .with_response(cut())
        .with_response(cut())
        .with_response(cut())
        .with_response(MockResponse::text("Should not reach this"));
    let config = AgentLoopConfig {
        system_prompt: "Test agent".to_string(),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let tools = vec![ToolDefinition::new(
        "read_file",
        "Read a file",
        serde_json::json!({"type": "object"}),
    )];

    let result = AgentLoop::new(config)
        .run(&provider, &executor, vec![Message::user("write it")], tools)
        .await
        .unwrap();

    assert_eq!(result.iterations, 6);
    assert!(result.output_truncated);
    assert!(!result.total_text.contains("Should not reach this"));
    let told = result
        .messages
        .iter()
        .filter(|msg| {
            msg.content.iter().any(|block| {
                matches!(block, ContentBlock::Text { text } if text.contains("before it made a tool call"))
            })
        })
        .count();
    assert_eq!(told, 4);
}

#[test]
fn test_tool_call_result_defaults() {
    let result = ToolCallResult::success("id", "content");
    assert!(!result.is_error);
    assert!(!result.stop_loop);

    let err = ToolCallResult::error("id", "error");
    assert!(err.is_error);
    assert!(!err.stop_loop);
}
