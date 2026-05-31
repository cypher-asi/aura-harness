//! Streaming-path test coverage retained after the Phase 7
//! buffered-transport deletion.
//!
//! The four `StreamReset` tests that lived here pre-Phase-7 pinned a
//! `BufferedTransport`-only contract: when the streaming SSE drain
//! threw mid-message, the buffered path would re-call `complete()`
//! (non-streaming) and synthesise a single `StreamReset` event before
//! re-emitting the authoritative text. The pump path has no such
//! fallback (`provider.complete_response_stream` is the only call
//! site), so those tests went away with `BufferedTransport`.
//!
//! The per-tool-call retry coverage below stays — it pins the
//! `StreamAbortedWithPartial` recovery the pump driver does in
//! [`super::stream_pump::driver`].

use aura_model_reasoner::{
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
            let events: Vec<Result<StreamEvent, ReasonerError>> = vec![
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
                    text: self.success_text.clone(),
                }),
                Ok(StreamEvent::ContentBlockStop { index: 0 }),
                Ok(StreamEvent::MessageDelta {
                    stop_reason: Some(StopReason::EndTurn),
                    output_tokens: 1,
                }),
                Ok(StreamEvent::MessageStop),
            ];
            return Ok(Box::pin(stream::iter(events)));
        }
        let events: Vec<Result<StreamEvent, ReasonerError>> = vec![
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
        ];
        Ok(Box::pin(stream::iter(events)))
    }

    async fn health_check(&self) -> bool {
        true
    }
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

#[tokio::test]
#[allow(clippy::await_holding_lock)] // intentional: serializes env var edits across async awaits
async fn stream_aborted_with_partial_retries_then_succeeds() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(5, 1, 2);

    let provider = FlakyPartialProvider::new(2, "recovered");
    let executor = NoOpExecutor;
    let agent = AgentLoop::new(pump_config());
    let (tx, rx) = mpsc::channel(1024);
    let messages = vec![Message::user("hello")];

    let result = agent
        .run_with_events(&provider, &executor, messages, vec![], Some(tx), None)
        .await
        .expect("retry should eventually succeed");
    assert_eq!(result.iterations, 1);

    let events = collect_events(rx).await;
    let retrying = events
        .iter()
        .filter(|e| matches!(e, AgentLoopEvent::ToolCallRetrying { .. }))
        .count();
    assert_eq!(
        retrying, 2,
        "expected exactly two ToolCallRetrying events, got: {retrying}"
    );

    let failed = events
        .iter()
        .any(|e| matches!(e, AgentLoopEvent::ToolCallFailed { .. }));
    assert!(!failed, "success path must not emit ToolCallFailed");

    let any_write_file_retry = events.iter().any(|e| match e {
        AgentLoopEvent::ToolCallRetrying { tool_name, .. } => tool_name == "write_file",
        _ => false,
    });
    assert!(
        any_write_file_retry,
        "retry events should carry the original tool_name (write_file)"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // intentional: serializes env var edits across async awaits
async fn stream_aborted_with_partial_exhausts_and_fails() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(2, 1, 2);

    let provider = FlakyPartialProvider::new(1_000, "never-used");
    let executor = NoOpExecutor;
    let agent = AgentLoop::new(pump_config());
    let (tx, rx) = mpsc::channel(1024);
    let messages = vec![Message::user("hello")];

    let result = agent
        .run_with_events(&provider, &executor, messages, vec![], Some(tx), None)
        .await;
    let _ = result;

    let events = collect_events(rx).await;
    let retrying = events
        .iter()
        .filter(|e| matches!(e, AgentLoopEvent::ToolCallRetrying { .. }))
        .count();
    assert_eq!(
        retrying, 2,
        "expected two retries before exhaustion, got: {retrying}"
    );

    let failed = events
        .iter()
        .filter(|e| matches!(e, AgentLoopEvent::ToolCallFailed { .. }))
        .count();
    assert_eq!(
        failed, 1,
        "expected exactly one ToolCallFailed after exhaustion, got: {failed}"
    );
}
