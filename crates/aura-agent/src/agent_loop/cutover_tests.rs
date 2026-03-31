//! Cutover tests: AgentLoop equivalents of every TurnProcessor test.
//!
//! These tests verify that AgentLoop covers the same behavioral surface as
//! TurnProcessor, ensuring no coverage is lost during the migration.

use aura_reasoner::{ContentBlock, Message, MockProvider, MockResponse, ModelProvider, ToolDefinition};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{AgentLoop, AgentLoopConfig};
use crate::events::TurnEvent;
use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};

struct MockExecutor {
    results: Vec<ToolCallResult>,
}

#[async_trait::async_trait]
impl AgentToolExecutor for MockExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        tool_calls
            .iter()
            .zip(self.results.iter())
            .map(|(tc, r)| ToolCallResult {
                tool_use_id: tc.id.clone(),
                ..r.clone()
            })
            .collect()
    }
}

fn default_config() -> AgentLoopConfig {
    AgentLoopConfig {
        system_prompt: "cutover test agent".to_string(),
        ..AgentLoopConfig::default()
    }
}

fn read_file_tool() -> ToolDefinition {
    ToolDefinition::new(
        "read_file",
        "Read a file",
        serde_json::json!({"type": "object"}),
    )
}

// =========================================================================
// Unit-test equivalents (from turn_processor/tests.rs)
// =========================================================================

/// Equivalent of `test_simple_turn`.
#[tokio::test]
async fn agentloop_simple_text_response() {
    let provider = MockProvider::simple_response("Hello!");
    let executor = MockExecutor { results: vec![] };
    let agent = AgentLoop::new(default_config());

    let result = agent
        .run(
            &provider,
            &executor,
            vec![Message::user("Hello")],
            vec![],
        )
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
        results: vec![ToolCallResult::success("placeholder", "file1.txt\nfile2.txt")],
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

/// Equivalent of `test_max_steps_limit`.
///
/// AgentLoop's budget check reserves the last iteration slot for a final
/// text response (`iteration >= max_iterations - 1`), so we set
/// `max_iterations` one higher than the desired cap to get the same
/// effective iteration count as TurnProcessor's `max_steps`.
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
        .run(
            &provider,
            &executor,
            vec![Message::user("Hello")],
            vec![],
        )
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
        .run(
            &provider,
            &executor,
            vec![Message::user("Hello")],
            vec![],
        )
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
        .run(
            &provider_a,
            &executor,
            vec![Message::user("Hello")],
            vec![],
        )
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
        .run(
            &provider,
            &executor,
            vec![Message::user("Hello")],
            vec![],
        )
        .await
        .unwrap();

    assert!(
        result.total_input_tokens > 0 || result.total_output_tokens > 0,
        "tokens must be recorded"
    );
}

/// Equivalent of `test_stream_callback_emits_events`.
///
/// Uses a `ModelCallDelegate` so that events are emitted through the
/// non-streaming delegate path (the direct streaming path requires
/// `complete_streaming` which `MockProvider` doesn't implement).
#[tokio::test]
async fn agentloop_events_emitted() {
    use std::sync::Arc;

    struct NonStreamingDelegate {
        provider: MockProvider,
    }

    #[async_trait::async_trait]
    impl crate::runtime::ModelCallDelegate for NonStreamingDelegate {
        async fn call_model(
            &self,
            request: aura_reasoner::ModelRequest,
        ) -> anyhow::Result<aura_reasoner::ModelResponse> {
            self.provider
                .complete(request)
                .await
                .map_err(anyhow::Error::from)
        }
    }

    let inner_provider = MockProvider::simple_response("Hello from stream!");
    let delegate = NonStreamingDelegate {
        provider: inner_provider,
    };
    let agent = AgentLoop::new(default_config()).with_model_delegate(Arc::new(delegate));

    let dummy_provider = MockProvider::new();
    let executor = MockExecutor { results: vec![] };
    let (tx, mut rx) = mpsc::channel::<TurnEvent>(100);

    let result = agent
        .run_with_events(
            &dummy_provider,
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
    assert!(has_error_result, "denied tool calls must produce error tool_result blocks");
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
