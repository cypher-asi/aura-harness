//! Contract tests for AgentLoop behavior that must hold across all refactor phases.

use aura_reasoner::{
    ContentBlock, Message, MockProvider, MockResponse, StopReason, ToolDefinition, Usage,
};
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
        system_prompt: "contract test agent".to_string(),
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

#[tokio::test]
async fn contract_agent_loop_drives_full_turn() {
    let executor = MockExecutor {
        results: vec![ToolCallResult::success("placeholder", "file contents here")],
    };

    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "tool_1",
            "read_file",
            serde_json::json!({"path": "test.txt"}),
        ))
        .with_response(MockResponse::text("Here is the summary."));

    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("Read test.txt and summarize")];
    let tools = vec![read_file_tool()];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(result.iterations, 2);
    assert!(result.total_text.contains("Here is the summary."));
    assert!(!result.timed_out);
    assert!(!result.stalled);
    assert!(result.llm_error.is_none());
}

#[tokio::test]
async fn contract_every_tool_use_gets_result() {
    let executor = MockExecutor {
        results: vec![
            ToolCallResult::success("placeholder", "contents of foo.rs"),
            ToolCallResult::success("placeholder", "contents of bar.rs"),
        ],
    };

    let multi_tool_response = MockResponse {
        stop_reason: StopReason::ToolUse,
        content: vec![
            ContentBlock::tool_use("tool_a", "read_file", serde_json::json!({"path": "foo.rs"})),
            ContentBlock::tool_use("tool_b", "read_file", serde_json::json!({"path": "bar.rs"})),
        ],
        usage: Usage::new(100, 50),
    };

    let provider = MockProvider::new()
        .with_response(multi_tool_response)
        .with_response(MockResponse::text("Both files read."));

    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("Read foo.rs and bar.rs")];
    let tools = vec![read_file_tool()];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    let tool_result_count = result
        .messages
        .iter()
        .flat_map(|msg| msg.content.iter())
        .filter(|block| matches!(block, ContentBlock::ToolResult { .. }))
        .count();

    assert_eq!(
        tool_result_count, 2,
        "Every tool_use block must have a corresponding tool_result"
    );
}

#[tokio::test]
async fn contract_policy_denied_tools_return_error() {
    let executor = MockExecutor {
        results: vec![ToolCallResult::error(
            "placeholder",
            "Policy denied: tool 'dangerous_tool' is not allowed",
        )],
    };

    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "tool_d",
            "dangerous_tool",
            serde_json::json!({"action": "rm -rf /"}),
        ))
        .with_response(MockResponse::text("Understood, I won't do that."));

    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("Do something dangerous")];
    let tools = vec![ToolDefinition::new(
        "dangerous_tool",
        "A dangerous tool",
        serde_json::json!({"type": "object"}),
    )];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    let has_error_result = result.messages.iter().any(|msg| {
        msg.content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { is_error: true, .. }))
    });
    assert!(
        has_error_result,
        "Denied tool calls must produce error tool_result blocks"
    );
    assert!(result.llm_error.is_none());
}

#[tokio::test]
async fn contract_cancellation_stops_loop() {
    let executor = MockExecutor { results: vec![] };
    let provider = MockProvider::simple_response("Should not appear");

    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("hello")];
    let tools = vec![];

    let token = CancellationToken::new();
    token.cancel();

    let result = agent
        .run_with_events(&provider, &executor, messages, tools, None, Some(token))
        .await
        .unwrap();

    assert_eq!(
        result.iterations, 0,
        "A pre-cancelled token must prevent any iterations"
    );
    assert!(
        result.total_text.is_empty(),
        "No text should be produced when cancelled before start"
    );
}

#[tokio::test]
async fn contract_streaming_events_ordered() {
    let executor = MockExecutor {
        results: vec![ToolCallResult::success("placeholder", "file data")],
    };

    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "tool_1",
            "read_file",
            serde_json::json!({"path": "test.txt"}),
        ))
        .with_response(MockResponse::text("Done."));

    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("Read test.txt")];
    let tools = vec![read_file_tool()];

    let (tx, mut rx) = mpsc::channel::<TurnEvent>(100);

    let result = agent
        .run_with_events(&provider, &executor, messages, tools, Some(tx), None)
        .await
        .unwrap();

    assert_eq!(result.iterations, 2);

    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    let first_tool_result_idx = events
        .iter()
        .position(|e| matches!(e, TurnEvent::ToolResult { .. }));
    let iteration_complete_for_tool_turn = events
        .iter()
        .position(|e| matches!(e, TurnEvent::IterationComplete { iteration: 0, .. }));

    assert!(
        first_tool_result_idx.is_some(),
        "Must emit at least one ToolResult event"
    );
    assert!(
        iteration_complete_for_tool_turn.is_some(),
        "Must emit IterationComplete for the tool-use iteration"
    );

    let tool_result_pos = first_tool_result_idx.unwrap();
    let iteration_complete_pos = iteration_complete_for_tool_turn.unwrap();

    assert!(
        tool_result_pos > iteration_complete_pos,
        "ToolResult (pos {tool_result_pos}) must come after IterationComplete \
         (pos {iteration_complete_pos}) for a tool-use iteration, because tool \
         execution happens after the model response is accumulated"
    );
}

#[tokio::test]
async fn contract_tool_cache_hit_matches_original() {
    let executor = MockExecutor {
        results: vec![ToolCallResult::success(
            "placeholder",
            "cached file contents",
        )],
    };

    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "tool_1",
            "read_file",
            serde_json::json!({"path": "same_file.txt"}),
        ))
        .with_response(MockResponse::tool_use(
            "tool_2",
            "read_file",
            serde_json::json!({"path": "same_file.txt"}),
        ))
        .with_response(MockResponse::text("Read it twice."));

    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("Read same_file.txt twice")];
    let tools = vec![read_file_tool()];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(result.iterations, 3);

    let tool_results: Vec<&ContentBlock> = result
        .messages
        .iter()
        .flat_map(|msg| msg.content.iter())
        .filter(|block| matches!(block, ContentBlock::ToolResult { .. }))
        .collect();

    assert!(
        tool_results.len() >= 2,
        "Both read_file calls must produce tool_result blocks (got {})",
        tool_results.len()
    );

    let contents: Vec<&str> = tool_results
        .iter()
        .filter_map(|block| {
            if let ContentBlock::ToolResult { content, .. } = block {
                match content {
                    aura_reasoner::ToolResultContent::Text(t) => Some(t.as_str()),
                    _ => None,
                }
            } else {
                None
            }
        })
        .collect();

    assert!(
        contents.len() >= 2,
        "Expected at least 2 text tool_result blocks"
    );
    assert_eq!(
        contents[0], contents[1],
        "Cache hit must produce the same content as the original execution"
    );
}
