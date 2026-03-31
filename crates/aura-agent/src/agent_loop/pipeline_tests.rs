//! Pipeline integration tests: `tool_use` → cache-split → execute → result → message.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use aura_reasoner::{
    ContentBlock, Message, MockProvider, MockResponse, ModelProvider, ModelRequest, ModelResponse,
    ReasonerError, StopReason, StreamContentType, StreamEvent, StreamEventStream, ToolDefinition,
    ToolResultContent, Usage,
};
use futures_util::stream;
use tokio::sync::mpsc;

use super::{AgentLoop, AgentLoopConfig};
use crate::events::AgentLoopEvent;
use crate::types::{AgentToolExecutor, AutoBuildResult, ToolCallInfo, ToolCallResult};

// ---------------------------------------------------------------------------
// Executors
// ---------------------------------------------------------------------------

struct SuccessExecutor;

#[async_trait::async_trait]
impl AgentToolExecutor for SuccessExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        tool_calls
            .iter()
            .map(|tc| ToolCallResult::success(&tc.id, "ok"))
            .collect()
    }
}

struct CountingExecutor {
    call_count: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl AgentToolExecutor for CountingExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        self.call_count
            .fetch_add(tool_calls.len(), Ordering::SeqCst);
        tool_calls
            .iter()
            .map(|tc| ToolCallResult::success(&tc.id, "result"))
            .collect()
    }
}

struct FailingWriteExecutor;

#[async_trait::async_trait]
impl AgentToolExecutor for FailingWriteExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        tool_calls
            .iter()
            .map(|tc| {
                if tc.name == "write_file" || tc.name == "edit_file" {
                    ToolCallResult::error(&tc.id, "Permission denied")
                } else {
                    ToolCallResult::success(&tc.id, "ok")
                }
            })
            .collect()
    }
}

struct BuildCheckExecutor;

#[async_trait::async_trait]
impl AgentToolExecutor for BuildCheckExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        tool_calls
            .iter()
            .map(|tc| ToolCallResult::success(&tc.id, "ok"))
            .collect()
    }

    async fn auto_build_check(&self) -> Option<AutoBuildResult> {
        Some(AutoBuildResult {
            success: false,
            output: "error[E0308]: mismatched types".to_string(),
            error_count: 1,
        })
    }
}

// ---------------------------------------------------------------------------
// StreamingMockProvider — emits proper tool-use stream events so the
// StreamAccumulator reconstructs ToolUse content blocks.
// ---------------------------------------------------------------------------

struct StreamingMockProvider {
    inner: MockProvider,
}

#[async_trait::async_trait]
impl ModelProvider for StreamingMockProvider {
    fn name(&self) -> &'static str {
        "streaming_mock"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        self.inner.complete(request).await
    }

    async fn complete_streaming(
        &self,
        request: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        let response = self.inner.complete(request).await?;
        let mut events: Vec<Result<StreamEvent, ReasonerError>> = Vec::new();

        events.push(Ok(StreamEvent::MessageStart {
            message_id: "msg_test".to_string(),
            model: "mock-model".to_string(),
            input_tokens: Some(response.usage.input_tokens),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }));

        for (idx, block) in response.message.content.iter().enumerate() {
            let index = idx as u32;
            match block {
                ContentBlock::Text { text } => {
                    events.push(Ok(StreamEvent::ContentBlockStart {
                        index,
                        content_type: StreamContentType::Text,
                    }));
                    events.push(Ok(StreamEvent::TextDelta { text: text.clone() }));
                    events.push(Ok(StreamEvent::ContentBlockStop { index }));
                }
                ContentBlock::ToolUse { id, name, input } => {
                    events.push(Ok(StreamEvent::ContentBlockStart {
                        index,
                        content_type: StreamContentType::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                        },
                    }));
                    let json = serde_json::to_string(input).unwrap_or_default();
                    events.push(Ok(StreamEvent::InputJsonDelta { partial_json: json }));
                    events.push(Ok(StreamEvent::ContentBlockStop { index }));
                }
                ContentBlock::Thinking { thinking, .. } => {
                    events.push(Ok(StreamEvent::ContentBlockStart {
                        index,
                        content_type: StreamContentType::Thinking,
                    }));
                    events.push(Ok(StreamEvent::ThinkingDelta {
                        thinking: thinking.clone(),
                    }));
                    events.push(Ok(StreamEvent::ContentBlockStop { index }));
                }
                _ => {}
            }
        }

        events.push(Ok(StreamEvent::MessageDelta {
            stop_reason: Some(response.stop_reason),
            output_tokens: response.usage.output_tokens,
        }));
        events.push(Ok(StreamEvent::MessageStop));

        Ok(Box::pin(stream::iter(events)))
    }

    async fn health_check(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_config() -> AgentLoopConfig {
    AgentLoopConfig {
        system_prompt: "pipeline test agent".to_string(),
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

fn write_file_tool() -> ToolDefinition {
    ToolDefinition::new(
        "write_file",
        "Write a file",
        serde_json::json!({"type": "object"}),
    )
}

async fn collect_events(mut rx: mpsc::Receiver<AgentLoopEvent>) -> Vec<AgentLoopEvent> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pipeline_full_tool_execution_flow() {
    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "tool_1",
            "read_file",
            serde_json::json!({"path": "test.txt"}),
        ))
        .with_response(MockResponse::text("done"));

    let executor = SuccessExecutor;
    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("Read test.txt")];
    let tools = vec![read_file_tool()];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(result.iterations, 2);

    let has_tool_result = result.messages.iter().any(|msg| {
        msg.content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
    });
    assert!(has_tool_result, "Messages must contain a tool_result block");
    assert!(
        result.total_text.contains("done"),
        "Final text must contain 'done'"
    );
}

#[tokio::test]
async fn pipeline_write_success_clears_cache() {
    let counter = Arc::new(AtomicUsize::new(0));
    let executor = CountingExecutor {
        call_count: Arc::clone(&counter),
    };

    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "t1",
            "read_file",
            serde_json::json!({"path": "test.txt"}),
        ))
        .with_response(MockResponse::tool_use(
            "t2",
            "write_file",
            serde_json::json!({"path": "out.rs", "content": "fn main() {}"}),
        ))
        .with_response(MockResponse::tool_use(
            "t3",
            "read_file",
            serde_json::json!({"path": "test.txt"}),
        ))
        .with_response(MockResponse::text("done"));

    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("read, write, read again")];
    let tools = vec![read_file_tool(), write_file_tool()];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(result.iterations, 4);
    assert_eq!(
        counter.load(Ordering::SeqCst),
        3,
        "read_file executed twice (cache cleared by write) + one write = 3 total"
    );
}

/// Sends 3 `write_file` calls in a single iteration to reach the failure
/// threshold (`WRITE_FAILURE_BLOCK_THRESHOLD` = 3) in one round, then
/// verifies the next write is blocked before the stall detector fires.
#[tokio::test]
async fn pipeline_blocking_detection_triggers() {
    let batch_response = MockResponse {
        stop_reason: StopReason::ToolUse,
        content: vec![
            ContentBlock::tool_use(
                "t1",
                "write_file",
                serde_json::json!({"path": "test.rs", "content": "a"}),
            ),
            ContentBlock::tool_use(
                "t2",
                "write_file",
                serde_json::json!({"path": "test.rs", "content": "b"}),
            ),
            ContentBlock::tool_use(
                "t3",
                "write_file",
                serde_json::json!({"path": "test.rs", "content": "c"}),
            ),
        ],
        usage: Usage::new(100, 50),
    };

    let provider = MockProvider::new()
        .with_response(batch_response)
        .with_default_response(MockResponse::tool_use(
            "t4",
            "write_file",
            serde_json::json!({"path": "test.rs", "content": "d"}),
        ));

    let executor = FailingWriteExecutor;
    let config = AgentLoopConfig {
        max_iterations: 3,
        ..default_config()
    };
    let agent = AgentLoop::new(config);
    let messages = vec![Message::user("write test.rs")];
    let tools = vec![write_file_tool()];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    let has_blocked = result.messages.iter().any(|msg| {
        msg.content.iter().any(|block| match block {
            ContentBlock::ToolResult {
                content: ToolResultContent::Text(text),
                ..
            } => text.contains("[BLOCKED]"),
            _ => false,
        })
    });
    assert!(
        has_blocked,
        "At least one tool result must contain [BLOCKED]"
    );
}

#[tokio::test]
async fn pipeline_stall_detection_stops_loop() {
    let provider = MockProvider::new().with_default_response(MockResponse::tool_use(
        "tw",
        "write_file",
        serde_json::json!({"path": "same_file.rs", "content": "bad code"}),
    ));

    let executor = FailingWriteExecutor;
    let config = AgentLoopConfig {
        max_iterations: 10,
        ..default_config()
    };
    let agent = AgentLoop::new(config);
    let messages = vec![Message::user("write same_file.rs")];
    let tools = vec![write_file_tool()];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert!(
        result.stalled,
        "Loop should be terminated by stall detection"
    );
    assert_eq!(
        result.iterations,
        crate::constants::STALL_STREAK_THRESHOLD,
        "Loop should stop after exactly STALL_STREAK_THRESHOLD iterations"
    );
}

#[tokio::test]
async fn pipeline_every_tool_emits_result_event() {
    let inner = MockProvider::new()
        .with_response(MockResponse {
            stop_reason: StopReason::ToolUse,
            content: vec![
                ContentBlock::tool_use(
                    "t1",
                    "read_file",
                    serde_json::json!({"path": "a.txt"}),
                ),
                ContentBlock::tool_use(
                    "t2",
                    "read_file",
                    serde_json::json!({"path": "b.txt"}),
                ),
            ],
            usage: Usage::new(100, 50),
        })
        .with_response(MockResponse::text("done"));

    let provider = StreamingMockProvider { inner };
    let executor = SuccessExecutor;
    let agent = AgentLoop::new(default_config());

    let (tx, rx) = mpsc::channel(1024);
    let messages = vec![Message::user("read two files")];
    let tools = vec![read_file_tool()];

    let result = agent
        .run_with_events(&provider, &executor, messages, tools, Some(tx), None)
        .await
        .unwrap();

    assert_eq!(result.iterations, 2);

    let events = collect_events(rx).await;
    let tool_result_count = events
        .iter()
        .filter(|e| matches!(e, AgentLoopEvent::ToolResult { .. }))
        .count();

    assert_eq!(
        tool_result_count, 2,
        "Each tool execution must emit a ToolResult event"
    );
}

#[tokio::test]
async fn pipeline_auto_build_after_write() {
    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "t1",
            "write_file",
            serde_json::json!({"path": "src/main.rs", "content": "fn main() {}"}),
        ))
        .with_response(MockResponse::text("done"));

    let executor = BuildCheckExecutor;
    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("write src/main.rs")];
    let tools = vec![write_file_tool()];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(result.iterations, 2);

    let has_build_failure = result.messages.iter().any(|msg| {
        msg.content.iter().any(|block| {
            if let ContentBlock::Text { text } = block {
                text.contains("Build check failed") && text.contains("error[E0308]")
            } else {
                false
            }
        })
    });
    assert!(
        has_build_failure,
        "Messages must contain the build failure output"
    );
}
