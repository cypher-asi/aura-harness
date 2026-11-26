use super::*;

/// Equivalent of `test_process_step_respects_model_override`.
#[tokio::test]
async fn agentloop_model_override() {
    let provider = MockProvider::simple_response("Overridden.");
    let executor = MockExecutor { results: vec![] };

    let config = AgentLoopConfig {
        model_override: Some("override-model".to_string()),
        ..default_config()
    };
    let agent = AgentLoop::new(config);

    let result = agent
        .run(&provider, &executor, vec![Message::user("Hello")], vec![])
        .await
        .unwrap();

    assert_eq!(result.iterations, 1);
    assert!(result.llm_error.is_none());
}

/// Equivalent of `test_run_turn_loop_backward_compat`.
#[tokio::test]
async fn agentloop_run_and_run_with_events_equivalent() {
    let provider_a = MockProvider::simple_response("Hello A!");
    let provider_b = MockProvider::simple_response("Hello B!");
    let executor = MockExecutor { results: vec![] };

    let config = default_config();
    let agent = AgentLoop::new(config);

    let result_run = agent
        .run(&provider_a, &executor, vec![Message::user("Hello")], vec![])
        .await
        .unwrap();

    let result_events = agent
        .run_with_events(
            &provider_b,
            &executor,
            vec![Message::user("Hello")],
            vec![],
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(result_run.iterations, result_events.iterations);
    assert_eq!(result_run.iterations, 1);
    assert!(result_run.llm_error.is_none());
    assert!(result_events.llm_error.is_none());
}

/// Equivalent of `test_multiple_sequential_tool_calls`.
#[tokio::test]
async fn agentloop_multiple_sequential_tools() {
    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "tool_1",
            "read_file",
            serde_json::json!({"path": "."}),
        ))
        .with_response(MockResponse::tool_use(
            "tool_2",
            "read_file",
            serde_json::json!({"path": "file.txt"}),
        ))
        .with_response(MockResponse::text("All done."));

    let executor = MockExecutor {
        results: vec![
            ToolCallResult::success("placeholder", "dir listing"),
            ToolCallResult::success("placeholder", "file contents"),
        ],
    };

    let agent = AgentLoop::new(default_config());

    let result = agent
        .run(
            &provider,
            &executor,
            vec![Message::user("Read files")],
            vec![read_file_tool()],
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 3);
    assert!(result.total_text.contains("All done."));
}

/// Equivalent of `test_max_steps_budget_enforcement`.
///
/// See [`agentloop_max_iterations_stops`] for why `max_iterations` is N+1.
#[tokio::test]
async fn agentloop_max_iterations_budget() {
    let provider = MockProvider::new().with_default_response(MockResponse::tool_use(
        "tool_loop",
        "read_file",
        serde_json::json!({"path": "."}),
    ));

    let executor = MockExecutor {
        results: vec![ToolCallResult::success("placeholder", "ok")],
    };

    let config = AgentLoopConfig {
        max_iterations: 3,
        ..default_config()
    };
    let agent = AgentLoop::new(config);

    let result = agent
        .run(
            &provider,
            &executor,
            vec![Message::user("Loop forever")],
            vec![read_file_tool()],
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 2);
}

/// Equivalent of `test_cancellation_stops_turn`.
#[tokio::test]
async fn agentloop_cancellation_pre_cancel() {
    let provider = MockProvider::simple_response("Should not appear");
    let executor = MockExecutor { results: vec![] };
    let agent = AgentLoop::new(default_config());

    let token = CancellationToken::new();
    token.cancel();

    let result = agent
        .run_with_events(
            &provider,
            &executor,
            vec![Message::user("Do work")],
            vec![],
            None,
            Some(token),
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 0);
    assert!(result.total_text.is_empty());
}

/// Equivalent of `test_process_turn_with_messages_entry_point`.
#[tokio::test]
async fn agentloop_messages_api() {
    let provider = MockProvider::simple_response("Hello via messages!");
    let executor = MockExecutor { results: vec![] };
    let agent = AgentLoop::new(default_config());

    let messages = vec![
        Message::user("First message"),
        Message::assistant("Prior response"),
        Message::user("Follow-up"),
    ];

    let result = agent
        .run(&provider, &executor, messages, vec![])
        .await
        .unwrap();

    assert_eq!(result.iterations, 1);
    assert!(result.total_text.contains("Hello via messages!"));
    assert!(result.llm_error.is_none());
}

/// Equivalent of `test_turn_result_token_accounting`.
#[tokio::test]
async fn agentloop_token_accounting() {
    let provider = MockProvider::simple_response("Hello!");
    let executor = MockExecutor { results: vec![] };
    let agent = AgentLoop::new(default_config());

    let result = agent
        .run(&provider, &executor, vec![Message::user("Hello")], vec![])
        .await
        .unwrap();

    assert!(
        result.total_input_tokens > 0 || result.total_output_tokens > 0,
        "tokens must be recorded"
    );
}

/// Equivalent of `test_stream_callback_emits_events`.
///
/// Uses the streaming path with `event_tx` to verify events are emitted.
/// `MockProvider` inherits the default `complete_streaming` which wraps
/// `complete()` in stream events, so this works without a real streaming
/// provider.
#[tokio::test]
async fn agentloop_events_emitted() {
    let provider = MockProvider::simple_response("Hello from stream!");
    let agent = AgentLoop::new(default_config());

    let executor = MockExecutor { results: vec![] };
    let (tx, mut rx) = mpsc::channel::<TurnEvent>(100);

    let result = agent
        .run_with_events(
            &provider,
            &executor,
            vec![Message::user("Hello")],
            vec![],
            Some(tx),
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 1);

    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    let has_iteration_complete = events
        .iter()
        .any(|e| matches!(e, TurnEvent::IterationComplete { .. }));
    assert!(
        has_iteration_complete,
        "expected at least one IterationComplete event"
    );
}
