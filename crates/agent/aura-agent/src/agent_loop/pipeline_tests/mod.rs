//! Pipeline integration tests: `tool_use` → cache-split → execute → result → message.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use aura_model::{
    ContentBlock, Message, MockProvider, MockResponse, ModelProvider, ModelRequest, ModelResponse,
    ReasonerError, StopReason, StreamContentType, StreamEvent, StreamEventStream, ToolDefinition,
    ToolResultContent, Usage,
};

use futures_util::stream;

use tokio::sync::{mpsc, Notify};

use tokio_util::sync::CancellationToken;

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
            let index = u32::try_from(idx).expect("content block index fits in u32");
            events.extend(content_block_events(index, block));
        }

        events.push(Ok(StreamEvent::MessageDelta {
            stop_reason: Some(response.stop_reason),
            output_tokens: response.usage.output_tokens,
            input_tokens: None,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }));
        events.push(Ok(StreamEvent::MessageStop));

        Ok(Box::pin(stream::iter(events)))
    }

    async fn health_check(&self) -> bool {
        true
    }
}

/// Stream events that reconstruct one content block (empty for block kinds the mock doesn't stream).
fn content_block_events(
    index: u32,
    block: &ContentBlock,
) -> Vec<Result<StreamEvent, ReasonerError>> {
    let mut events: Vec<Result<StreamEvent, ReasonerError>> = Vec::new();
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
    events
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_config() -> AgentLoopConfig {
    AgentLoopConfig {
        system_prompt: "pipeline test agent".to_string(),
        ..AgentLoopConfig::for_agent("claude-test-model")
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

/// Executor that signals start via a `Notify`, then awaits an
/// `unblock` signal before returning. Lets the test deterministically
/// observe "tool execution has started" so it can fire the
/// cancellation token at exactly the right moment.
struct BlockingExecutor {
    started: Arc<Notify>,
    unblock: Arc<Notify>,
}

#[async_trait::async_trait]
impl AgentToolExecutor for BlockingExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        self.started.notify_one();
        self.unblock.notified().await;
        tool_calls
            .iter()
            .map(|tc| ToolCallResult::success(&tc.id, "executor returned despite cancellation"))
            .collect()
    }
}

mod auto_build;
mod cancellation;
mod heartbeat;
mod markup_retry;
mod tool_flow;
