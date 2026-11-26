use super::*;

/// Counterpart to `turn_continues_while_needs_follow_up_true`: a
/// single `EndTurn` (no tool calls) must break the turn loop on the
/// first sampling. Pinned so the polarity flip cannot accidentally
/// re-introduce an off-by-one extra sampling on the "model already
/// said stop" path.
#[tokio::test]
async fn turn_breaks_when_model_says_stop_and_no_continuation() {
    let executor = MockExecutor { results: vec![] };
    let provider = MockProvider::simple_response("Nothing to do.");

    let config = AgentLoopConfig {
        system_prompt: "test agent".to_string(),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let agent = AgentLoop::new(config);
    let messages = vec![Message::user("are you there?")];
    let tools = vec![];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(
        result.iterations, 1,
        "a clean EndTurn with no follow-up must break the turn loop \
         on the first sampling"
    );
    assert!(result.total_text.contains("Nothing to do."));
    assert!(!result.stalled);
    assert!(result.llm_error.is_none());
}

#[tokio::test]
async fn unfinished_action_gets_one_recovery_attempt() {
    let executor = MockExecutor {
        results: vec![ToolCallResult::success("unused", "contents")],
    };
    let provider = MockProvider::new()
        .with_response(MockResponse::text("I'll read the file now."))
        .with_response(MockResponse::tool_use(
            "read",
            "read_file",
            serde_json::json!({"path": "test.txt"}),
        ))
        .with_response(MockResponse::text("The file contains the expected value."));
    let agent = AgentLoop::new(AgentLoopConfig::for_agent("claude-test-model"));
    let tools = vec![ToolDefinition::new(
        "read_file",
        "Read",
        serde_json::json!({"type":"object"}),
    )];
    let result = agent
        .run(
            &provider,
            &executor,
            vec![Message::user("check test.txt")],
            tools,
        )
        .await
        .unwrap();
    assert_eq!(result.iterations, 3);
    assert!(result.total_text.contains("expected value"));
}

#[tokio::test]
async fn unfinished_action_recovery_is_bounded() {
    let executor = MockExecutor { results: vec![] };
    let provider = MockProvider::new()
        .with_response(MockResponse::text("I'll read the file now."))
        .with_response(MockResponse::text("Doing it now."))
        .with_response(MockResponse::text("must not be sampled"));
    let agent = AgentLoop::new(AgentLoopConfig::for_agent("claude-test-model"));
    let tools = vec![ToolDefinition::new(
        "read_file",
        "Read",
        serde_json::json!({"type":"object"}),
    )];
    let (tx, mut rx) = mpsc::channel(64);
    let result = agent
        .run_with_events(
            &provider,
            &executor,
            vec![Message::user("check test.txt")],
            tools,
            Some(tx),
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.iterations, 2);
    assert!(!result.total_text.contains("must not be sampled"));
    assert!(
        std::iter::from_fn(|| rx.try_recv().ok()).any(|e| matches!(e,
        AgentLoopEvent::Progress { stage, .. } if stage == "turn_ended_unfinished_action"))
    );
}

#[tokio::test]
async fn planning_and_blocked_answers_do_not_trigger_action_recovery() {
    for (request, answer) in [
        ("plan the investigation", "I'll read the file now."),
        (
            "check test.txt",
            "I'll read the file once you grant access.",
        ),
        (
            "check test.txt",
            "I'll read the file. Can you provide its path?",
        ),
        ("check test.txt", "Done. The file is valid."),
    ] {
        let provider = MockProvider::new().with_response(MockResponse::text(answer));
        let executor = MockExecutor { results: vec![] };
        let agent = AgentLoop::new(AgentLoopConfig::for_agent("claude-test-model"));
        let tools = vec![ToolDefinition::new(
            "read_file",
            "Read",
            serde_json::json!({"type":"object"}),
        )];
        let result = agent
            .run(&provider, &executor, vec![Message::user(request)], tools)
            .await
            .unwrap();
        assert_eq!(result.iterations, 1, "request={request}, answer={answer}");
    }
}

#[tokio::test]
async fn identical_failed_edits_stop_with_an_explicit_error() {
    let executor = MockExecutor {
        results: vec![ToolCallResult {
            tool_use_id: "unused".to_string(),
            content: "The specified text was not found".to_string(),
            is_error: true,
            kind: aura_core_types::ToolResultKind::AgentError,
            stop_loop: false,
            file_changes: vec![],
            image: None,
        }],
    };
    let mut provider = MockProvider::new();
    for i in 0..5 {
        provider = provider.with_response(MockResponse::tool_use(
            format!("edit_{i}"),
            "edit_file",
            serde_json::json!({"path":"test.txt", "old_text":"missing", "new_text":"replacement"}),
        ));
    }
    let agent = AgentLoop::new(AgentLoopConfig::for_agent("claude-test-model"));
    let tools = vec![ToolDefinition::new(
        "edit_file",
        "Edit",
        serde_json::json!({"type":"object"}),
    )];
    let result = agent
        .run(
            &provider,
            &executor,
            vec![Message::user("fix test.txt")],
            tools,
        )
        .await
        .unwrap();
    assert_eq!(result.iterations, 4);
    assert!(result.stalled);
    assert!(result
        .llm_error
        .as_deref()
        .unwrap()
        .contains("repeated failed edit loop"));
    assert!(result
        .messages
        .last()
        .unwrap()
        .content
        .iter()
        .any(|b| matches!(b,
        ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "edit_3")));
}

/// Per-task hard ceiling regression: a misconfigured
/// `max_turns_per_task = 0` must surface as a typed
/// `AgentError::TurnBudgetExceeded` rather than silently returning
/// an empty result. Pinned because the cap trips inside
/// `task::run_task` before the first `run_turn` call and the Err
/// propagates up through `AgentLoop::run` — callers that rely on
/// the pre-E.1 "always-Ok" contract for normal LLM/tool errors must
/// still see a structured failure for budget overruns.
#[tokio::test]
async fn turn_budget_exceeded_surfaces_typed_error_when_max_turns_zero() {
    let executor = MockExecutor { results: vec![] };
    let provider = MockProvider::simple_response("never reached");

    let config = AgentLoopConfig {
        system_prompt: "test".to_string(),
        max_turns_per_task: 0,
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let agent = AgentLoop::new(config);
    let messages = vec![Message::user("anything")];
    let tools = vec![];

    let err = agent
        .run(&provider, &executor, messages, tools)
        .await
        .expect_err("max_turns_per_task=0 must trip TurnBudgetExceeded");

    match err {
        crate::AgentError::TurnBudgetExceeded { limit, .. } => {
            assert_eq!(limit, 0, "zero-budget cap exposes the tripped limit");
        }
        other => panic!("expected TurnBudgetExceeded, got {other:?}"),
    }
}
