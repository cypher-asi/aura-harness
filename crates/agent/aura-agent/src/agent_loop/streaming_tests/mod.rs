//! Streaming-path test coverage retained after the Phase 7
//! buffered-transport deletion.
//!
//! Interrupted connections are retried as fresh streams with a bounded
//! budget and `StreamReset`, never by splicing partial responses or
//! replaying a request after a completed tool call was admitted.
//!
//! The per-tool-call retry coverage below stays — it pins the
//! `StreamAbortedWithPartial` recovery the pump driver does in
//! [`super::stream_pump::driver`].

use aura_model::{
    Message, ModelProvider, ModelRequest, ModelResponse, ProviderTrace, ReasonerError, StopReason,
    StreamContentType, StreamEvent, StreamEventStream, Usage,
};

use futures_util::stream;

use tokio::sync::mpsc;

use super::{AgentLoop, AgentLoopConfig};

use crate::events::AgentLoopEvent;

use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};

struct NoOpExecutor;

#[async_trait::async_trait]
impl AgentToolExecutor for NoOpExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        tool_calls
            .iter()
            .map(|tc| ToolCallResult::success(&tc.id, "ok"))
            .collect()
    }
}

fn pump_config() -> AgentLoopConfig {
    AgentLoopConfig {
        system_prompt: "streaming test agent".to_string(),
        ..AgentLoopConfig::for_agent("claude-test-model")
    }
}

async fn collect_events(mut rx: mpsc::Receiver<AgentLoopEvent>) -> Vec<AgentLoopEvent> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

// ---------------------------------------------------------------------------
// Per-tool-call streaming retry (StreamAbortedWithPartial)
// ---------------------------------------------------------------------------

/// Mock provider whose `complete_streaming` emits a `tool_use`
/// `content_block_start` + a partial `input_json_delta`, then a
/// mid-stream SSE `Error` event before `content_block_stop`. The
/// `StreamAccumulator` turns this into
/// `ReasonerError::StreamAbortedWithPartial` inside the agent's
/// streaming call -- exactly the retry trigger we want to test.
///
/// The `fail_count` counter decides how many attempts to fail before
/// finally emitting a clean `MessageStop`; `usize::MAX` means "always
/// fail" (retry-budget-exhaustion test).
struct FlakyPartialProvider {
    fail_count: std::sync::atomic::AtomicUsize,
    success_text: String,
}

impl FlakyPartialProvider {
    fn new(fail_count: usize, text: &str) -> Self {
        Self {
            fail_count: std::sync::atomic::AtomicUsize::new(fail_count),
            success_text: text.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl ModelProvider for FlakyPartialProvider {
    fn name(&self) -> &'static str {
        "flaky-partial-test"
    }

    async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        Ok(ModelResponse::new(
            StopReason::EndTurn,
            Message::assistant(&self.success_text),
            Usage::new(1, 1),
            ProviderTrace::new("test", 0),
        ))
    }

    async fn complete_streaming(
        &self,
        _request: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        let remaining = self
            .fail_count
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        if remaining == 0 {
            self.fail_count
                .store(0, std::sync::atomic::Ordering::SeqCst);
            let events = flaky_success_events(&self.success_text);
            return Ok(Box::pin(stream::iter(events)));
        }
        let events = flaky_partial_tool_events();
        Ok(Box::pin(stream::iter(events)))
    }

    async fn health_check(&self) -> bool {
        true
    }
}

/// A clean text answer: `MessageStart`, one text block, `MessageStop`.
fn flaky_success_events(success_text: &str) -> Vec<Result<StreamEvent, ReasonerError>> {
    vec![
        Ok(StreamEvent::MessageStart {
            message_id: "msg_ok".to_string(),
            model: "test".to_string(),
            input_tokens: Some(1),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::Text,
        }),
        Ok(StreamEvent::TextDelta {
            text: success_text.to_string(),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            output_tokens: 1,
            input_tokens: None,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

/// A `write_file` tool block whose input JSON is cut off by an SSE `Error`.
fn flaky_partial_tool_events() -> Vec<Result<StreamEvent, ReasonerError>> {
    vec![
        Ok(StreamEvent::MessageStart {
            message_id: "msg_fail".to_string(),
            model: "test".to_string(),
            input_tokens: Some(1),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::ToolUse {
                id: "toolu_partial".to_string(),
                name: "write_file".to_string(),
            },
        }),
        Ok(StreamEvent::InputJsonDelta {
            partial_json: "{\"path\":\"src/".to_string(),
        }),
        Ok(StreamEvent::Error {
            message: "overloaded_error: upstream flaked".to_string(),
            request_id: None,
        }),
    ]
}

/// The successful retry stream of [`DisconnectProvider`].
fn recovered_answer_events() -> Vec<Result<StreamEvent, ReasonerError>> {
    vec![
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::Text,
        }),
        Ok(StreamEvent::TextDelta {
            text: "recovered answer".into(),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            output_tokens: 1,
            input_tokens: None,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

/// A completed `write_file` tool block at index 1.
fn completed_write_tool_events() -> [Result<StreamEvent, ReasonerError>; 3] {
    [
        Ok(StreamEvent::ContentBlockStart {
            index: 1,
            content_type: StreamContentType::ToolUse {
                id: "toolu_write".into(),
                name: "write_file".into(),
            },
        }),
        Ok(StreamEvent::InputJsonDelta {
            partial_json: "{}".into(),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 1 }),
    ]
}

/// Shared lock so the retry-budget tests can swap
/// `aura_config::reasoner().llm_retry` without racing each other or
/// the config tests in `aura-reasoner`.
static STREAM_RETRY_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn install_retry(max_retries: u32, initial_ms: u64, cap_ms: u64) -> aura_config::ConfigGuard {
    let mut cfg = aura_config::current();
    cfg.reasoner.llm_retry.max_retries = max_retries;
    cfg.reasoner.llm_retry.backoff_initial = std::time::Duration::from_millis(initial_ms);
    cfg.reasoner.llm_retry.backoff_cap = std::time::Duration::from_millis(cap_ms);
    aura_config::install_for_test(cfg)
}

#[derive(Clone, Copy)]
enum DisconnectKind {
    BodyReset,
    OpenRequest,
    InvalidEvent,
    AfterTool,
    PartialAfterTool,
}

struct DisconnectProvider {
    calls: std::sync::atomic::AtomicUsize,
    failures: usize,
    kind: DisconnectKind,
}

impl DisconnectProvider {
    fn new(failures: usize, kind: DisconnectKind) -> Self {
        Self {
            calls: std::sync::atomic::AtomicUsize::new(0),
            failures,
            kind,
        }
    }
}

#[async_trait::async_trait]
impl ModelProvider for DisconnectProvider {
    fn name(&self) -> &'static str {
        "disconnect-test"
    }

    async fn health_check(&self) -> bool {
        true
    }

    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        panic!("disconnect recovery must not switch to a buffered request")
    }

    async fn complete_streaming(
        &self,
        _: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call >= self.failures {
            return Ok(Box::pin(stream::iter(recovered_answer_events())));
        }
        if matches!(self.kind, DisconnectKind::OpenRequest) {
            return Err(ReasonerError::Request(
                "connection reset before headers".into(),
            ));
        }
        let mut events = vec![
            Ok(StreamEvent::ContentBlockStart {
                index: 0,
                content_type: StreamContentType::Text,
            }),
            Ok(StreamEvent::TextDelta {
                text: "discard this partial answer".into(),
            }),
            Ok(StreamEvent::ContentBlockStop { index: 0 }),
        ];
        if matches!(
            self.kind,
            DisconnectKind::AfterTool | DisconnectKind::PartialAfterTool
        ) {
            events.extend(completed_write_tool_events());
        }
        if matches!(self.kind, DisconnectKind::PartialAfterTool) {
            events.push(Ok(StreamEvent::ContentBlockStart {
                index: 2,
                content_type: StreamContentType::ToolUse {
                    id: "toolu_partial".into(),
                    name: "edit_file".into(),
                },
            }));
        }
        if matches!(self.kind, DisconnectKind::InvalidEvent) {
            events.push(Ok(StreamEvent::Error {
                request_id: None,
                message: "invalid request".into(),
            }));
        } else {
            // The exact Windows body-read failure reported in AURA (test).
            events.push(Err(ReasonerError::Request("Stream error: request or response body error: error reading a body from connection: The remote host forcibly closed the existing connection. (os error 10054)".into())));
        }
        Ok(Box::pin(stream::iter(events)))
    }
}

mod connection_reset;
mod partial_retry;
