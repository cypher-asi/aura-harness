use super::*;

fn agent_with_prompt(system_prompt: &str) -> AgentLoop {
    AgentLoop::new(AgentLoopConfig {
        system_prompt: system_prompt.to_string(),
        ..AgentLoopConfig::for_agent("claude-test-model")
    })
}

fn object_tool(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition::new(name, description, serde_json::json!({"type": "object"}))
}

fn read_file_call(id: &str, path: &str) -> MockResponse {
    MockResponse::tool_use(id, "read_file", serde_json::json!({ "path": path }))
}

/// Successful `write_file` result that records the creation of `src/new.rs`.
fn new_file_write_result() -> ToolCallResult {
    ToolCallResult {
        tool_use_id: "call_write".to_string(),
        content: "wrote new.rs".to_string(),
        is_error: false,
        kind: aura_core_types::ToolResultKind::Ok,
        stop_loop: false,
        file_changes: vec![crate::types::FileChange {
            path: "src/new.rs".to_string(),
            kind: crate::types::FileChangeKind::Create,
            lines_added: 1,
            lines_removed: 0,
        }],
        image: None,
    }
}

#[tokio::test]
async fn chat_mode_endturn_terminates_immediately() {
    // Regression guard: a normal chat session ("read one file,
    // answer the question") exits cleanly on the first EndTurn.
    // Codex parity: the harness no longer intercepts empty
    // terminations, so this is just a straightforward EndTurn
    // termination test.
    //
    //   Iter 0: read_file (ToolUse)
    //   Iter 1: text only (EndTurn) -> exits IMMEDIATELY
    let executor = MockExecutor {
        results: vec![ToolCallResult::success("call_read", "fn foo() {}")],
    };

    let provider = MockProvider::new()
        .with_response(MockResponse {
            stop_reason: StopReason::ToolUse,
            content: vec![ContentBlock::tool_use(
                "call_read",
                "read_file",
                serde_json::json!({"path": "src/lib.rs"}),
            )],
            usage: Usage::new(100, 20),
        })
        .with_response(MockResponse {
            stop_reason: StopReason::EndTurn,
            content: vec![ContentBlock::text("It's a one-line stub.")],
            usage: Usage::new(150, 30),
        });

    let config = AgentLoopConfig {
        system_prompt: "test".to_string(),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let agent = AgentLoop::new(config);
    let messages = vec![Message::user("what does src/lib.rs contain?")];
    let tools = vec![ToolDefinition::new(
        "read_file",
        "Read a file",
        serde_json::json!({"type": "object"}),
    )];

    let (tx, mut rx) = mpsc::channel(64);
    let result = agent
        .run_with_events(&provider, &executor, messages, tools, Some(tx), None)
        .await
        .unwrap();

    assert_eq!(
        result.iterations, 2,
        "chat mode must exit on the EndTurn that follows the read"
    );
    while let Ok(event) = rx.try_recv() {
        if let AgentLoopEvent::Warning(msg) = event {
            assert!(
                !(msg.contains("ended your turn without writing")
                    || msg.contains("Second EndTurn without progress")
                    || msg.contains("Third EndTurn without progress")),
                "chat mode must not emit dev-loop intercept nudges; got: {msg}"
            );
        }
    }
}

// ------------------------------------------------------------------
// Codex parity: trust EndTurn (replacement tests)
// ------------------------------------------------------------------

/// Codex parity #1: a single `EndTurn` with no tool calls and no
/// writes must terminate the loop cleanly on the first iteration.
/// Pre-codex-parity the harness would intercept the empty EndTurn
/// and force-inject a continuation prompt up to ~24 times before
/// failing with `task_blocked`; the new behaviour trusts the model.
#[tokio::test]
async fn agentloop_endturn_terminates_without_writes() {
    let executor = MockExecutor { results: vec![] };
    let provider = MockProvider::new().with_response(MockResponse {
        stop_reason: StopReason::EndTurn,
        content: vec![ContentBlock::text("I'm done; nothing to write.")],
        usage: Usage::new(50, 10),
    });

    let agent = AgentLoop::new(AgentLoopConfig {
        system_prompt: "test".to_string(),
        ..AgentLoopConfig::for_agent("claude-test-model")
    });
    let messages = vec![Message::user("verify the bar is already implemented")];
    let tools = vec![ToolDefinition::new(
        "read_file",
        "Read a file",
        serde_json::json!({"type": "object"}),
    )];

    let (tx, mut rx) = mpsc::channel(64);
    let result = agent
        .run_with_events(&provider, &executor, messages, tools, Some(tx), None)
        .await
        .unwrap();

    assert_eq!(
        result.iterations, 1,
        "EndTurn must terminate the loop on the first sampling regardless of writes"
    );
    assert!(
        !result.stalled,
        "no stall flag without an explicit budget overrun"
    );
    assert!(
        result.llm_error.is_none(),
        "no llm_error envelope — the model owns the exit signal"
    );

    while let Ok(event) = rx.try_recv() {
        if let AgentLoopEvent::Warning(msg) = event {
            assert!(
                !msg.contains("<harness_continuation"),
                "no <harness_continuation> envelope must surface; got: {msg}"
            );
        }
    }
}

/// Codex parity #2: a write followed by `EndTurn` terminates on the
/// EndTurn turn (iteration 2). Confirms the loop continues after the
/// tool batch and exits cleanly on the next stop signal without
/// continuation injection.
#[tokio::test]
async fn agentloop_endturn_terminates_after_partial_write() {
    let executor = MockExecutor {
        results: vec![new_file_write_result()],
    };

    let provider = MockProvider::new()
        .with_response(MockResponse {
            stop_reason: StopReason::ToolUse,
            content: vec![ContentBlock::tool_use(
                "call_write",
                "write_file",
                serde_json::json!({"path": "src/new.rs", "content": "pub fn bar() {}"}),
            )],
            usage: Usage::new(100, 20),
        })
        .with_response(MockResponse {
            stop_reason: StopReason::EndTurn,
            content: vec![ContentBlock::text("Done after write.")],
            usage: Usage::new(150, 30),
        });

    let agent = agent_with_prompt("test");
    let messages = vec![Message::user("implement bar")];
    let tools = vec![object_tool("write_file", "Write a file")];

    let (tx, mut rx) = mpsc::channel(64);
    let result = agent
        .run_with_events(&provider, &executor, messages, tools, Some(tx), None)
        .await
        .unwrap();

    assert_eq!(
        result.iterations, 2,
        "first iter writes, second iter EndTurn terminates"
    );
    assert!(
        !result.file_changes.is_empty(),
        "the write must be recorded on the result"
    );
    assert!(result.llm_error.is_none());

    while let Ok(event) = rx.try_recv() {
        if let AgentLoopEvent::Warning(msg) = event {
            assert!(
                !msg.contains("<harness_continuation"),
                "no continuation envelopes after a clean EndTurn; got: {msg}"
            );
        }
    }
}

/// Codex parity #3: an `EndTurn` arriving without any `task_done`
/// call and without any writes still terminates cleanly. Pre-codex-
/// parity the absence of `task_done` would gate the EndTurn intercept
/// open and the loop would keep nudging.
#[tokio::test]
async fn agentloop_endturn_terminates_when_no_task_done_called() {
    let executor = MockExecutor { results: vec![] };
    let provider = MockProvider::new().with_response(MockResponse {
        stop_reason: StopReason::EndTurn,
        content: vec![ContentBlock::text("Looks fine; no edits needed.")],
        usage: Usage::new(30, 8),
    });

    let agent = AgentLoop::new(AgentLoopConfig {
        system_prompt: "test".to_string(),
        ..AgentLoopConfig::for_agent("claude-test-model")
    });
    let messages = vec![Message::user("check if foo handles None correctly")];
    let tools = vec![ToolDefinition::new(
        "task_done",
        "Signal task completion",
        serde_json::json!({"type": "object"}),
    )];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(result.iterations, 1);
    assert!(
        result.llm_error.is_none(),
        "no task_blocked llm_error — the harness no longer escalates on missing task_done"
    );
    assert!(
        !result.stalled,
        "stalled must stay false when the model itself chose to terminate"
    );
}

// ------------------------------------------------------------------
// Layer E.1 — nested loop topology (task -> turn -> sampling)
// ------------------------------------------------------------------

/// The turn loop must consume every `ToolUse` sampling without
/// breaking, then break cleanly on the trailing `EndTurn`. Codex-shape
/// regression: pinned because the polarity flip from
/// `for iteration { … if break { break; } }` to
/// `loop { … if !needs_follow_up { break; } }` would have been a
/// silent behavior change if `dispatch_stop_reason`'s "loop should
/// break" semantics had been mis-translated into `needs_follow_up`.
#[tokio::test]
async fn turn_continues_while_needs_follow_up_true() {
    let executor = MockExecutor {
        results: vec![
            ToolCallResult::success("ph1", "alpha"),
            ToolCallResult::success("ph2", "beta"),
            ToolCallResult::success("ph3", "gamma"),
        ],
    };

    let provider = MockProvider::new()
        .with_response(read_file_call("tc_1", "a.rs"))
        .with_response(read_file_call("tc_2", "b.rs"))
        .with_response(read_file_call("tc_3", "c.rs"))
        .with_response(MockResponse::text("Done after three reads."));

    let agent = agent_with_prompt("test agent");
    let messages = vec![Message::user("read three files")];
    let tools = vec![object_tool("read_file", "Read a file")];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(
        result.iterations, 4,
        "three ToolUse follow-ups plus one terminating EndTurn must \
         all run inside one turn — total sampling requests = 4"
    );
    assert!(
        result.total_text.contains("Done after three reads."),
        "the final EndTurn message must surface on the result"
    );
    assert!(
        result.llm_error.is_none(),
        "no llm_error expected on the happy path"
    );
}

/// Regression from a production AURA repair-agent run: the model
/// spent the whole turn reading files, then stopped with no assistant
/// text and no file changes. Tool calls alone are not a completed
/// user-visible response, so the no-op recovery must re-prompt once.
#[tokio::test]
async fn read_only_tool_loop_with_empty_endturn_gets_no_op_retry() {
    let executor = MockExecutor {
        results: vec![ToolCallResult::success("placeholder", "file contents")],
    };

    let provider = MockProvider::new()
        .with_response(read_file_call("tc_read", "src/main.rs"))
        .with_response(MockResponse {
            stop_reason: StopReason::EndTurn,
            content: vec![],
            usage: Usage::new(120, 0),
        })
        .with_response(MockResponse::text(
            "I found the issue and will patch it next.",
        ));

    let agent = agent_with_prompt("test agent");
    let messages = vec![Message::user("audit and fix this project")];
    let tools = vec![object_tool("read_file", "Read a file")];

    let (tx, mut rx) = mpsc::channel(64);
    let result = agent
        .run_with_events(&provider, &executor, messages, tools, Some(tx), None)
        .await
        .unwrap();

    assert_eq!(
        result.iterations, 3,
        "read-only tool use plus empty EndTurn must consume one retry and then finish"
    );
    assert!(
        result
            .total_text
            .contains("I found the issue and will patch it next."),
        "the retry response should become the visible result"
    );

    let mut saw_retry_progress = false;
    while let Ok(event) = rx.try_recv() {
        if let AgentLoopEvent::Progress { stage, .. } = event {
            saw_retry_progress |= stage == "turn_no_action_retry";
        }
    }
    assert!(
        saw_retry_progress,
        "the stream should explain that a no-action turn was retried"
    );
}
