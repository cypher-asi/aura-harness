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
            return Ok(Box::pin(stream::iter(vec![
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
                }),
                Ok(StreamEvent::MessageStop),
            ])));
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
            events.extend([
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
            ]);
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

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn stream_connection_reset_recovers_and_discards_partial_output() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(2, 1, 2);
    let provider = DisconnectProvider::new(1, DisconnectKind::BodyReset);
    let (tx, rx) = mpsc::channel(1024);
    let result = AgentLoop::new(pump_config())
        .run_with_events(
            &provider,
            &NoOpExecutor,
            vec![Message::user("hello")],
            vec![],
            Some(tx),
            None,
        )
        .await
        .expect("recovered run");
    assert!(result.llm_error.is_none(), "{:?}", result.llm_error);
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    let events = collect_events(rx).await;
    let reset = events
        .iter()
        .position(|e| matches!(e, AgentLoopEvent::StreamReset { .. }))
        .expect("reset partial UI content");
    assert!(
        matches!(&events[reset], AgentLoopEvent::StreamReset { text_bytes, thinking_bytes, .. }
        if *text_bytes == "discard this partial answer".len() && *thinking_bytes == 0)
    );
    assert!(events[..reset]
        .iter()
        .any(|e| matches!(e, AgentLoopEvent::TextDelta(t) if t.contains("discard"))));
    assert!(events[reset + 1..]
        .iter()
        .any(|e| matches!(e, AgentLoopEvent::TextDelta(t) if t == "recovered answer")));
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentLoopEvent::ToolCallFailed { .. })));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn stream_connection_reset_exhausts_bounded_budget() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(2, 1, 2);
    let provider = DisconnectProvider::new(usize::MAX, DisconnectKind::BodyReset);
    let (tx, rx) = mpsc::channel(1024);
    let result = AgentLoop::new(pump_config())
        .run_with_events(
            &provider,
            &NoOpExecutor,
            vec![Message::user("hello")],
            vec![],
            Some(tx),
            None,
        )
        .await
        .expect("errors carried by loop result");
    assert!(result
        .llm_error
        .as_deref()
        .is_some_and(|s| s.contains("10054")));
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    let events = collect_events(rx).await;
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentLoopEvent::StreamReset { .. }))
            .count(),
        2
    );
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentLoopEvent::ToolCallFailed { .. })));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn stream_open_connection_error_recovers() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(2, 1, 2);
    let provider = DisconnectProvider::new(1, DisconnectKind::OpenRequest);
    let result = AgentLoop::new(pump_config())
        .run(
            &provider,
            &NoOpExecutor,
            vec![Message::user("hello")],
            vec![],
        )
        .await
        .expect("recovered request");
    assert!(result.llm_error.is_none());
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn stream_invalid_event_is_not_retried() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(2, 1, 2);
    let provider = DisconnectProvider::new(1, DisconnectKind::InvalidEvent);
    let result = AgentLoop::new(pump_config())
        .run(
            &provider,
            &NoOpExecutor,
            vec![Message::user("hello")],
            vec![],
        )
        .await
        .expect("errors carried by loop result");
    assert!(result.llm_error.is_some());
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn stream_disconnect_after_completed_tool_is_not_replayed() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(2, 1, 2);
    for kind in [DisconnectKind::AfterTool, DisconnectKind::PartialAfterTool] {
        let provider = DisconnectProvider::new(1, kind);
        let result = AgentLoop::new(pump_config())
            .run(
                &provider,
                &NoOpExecutor,
                vec![Message::user("hello")],
                vec![],
            )
            .await
            .expect("errors carried by loop result");
        assert!(result.llm_error.is_some());
        assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn stream_connection_retry_backoff_honors_cancellation() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(2, 30_000, 30_000);
    let provider = DisconnectProvider::new(usize::MAX, DisconnectKind::BodyReset);
    let token = tokio_util::sync::CancellationToken::new();
    let (tx, mut rx) = mpsc::channel(1024);
    let watch_token = token.clone();
    let watcher = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if matches!(event, AgentLoopEvent::Progress { stage, .. } if stage == "model_retrying")
            {
                watch_token.cancel();
                break;
            }
        }
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        AgentLoop::new(pump_config()).run_with_events(
            &provider,
            &NoOpExecutor,
            vec![Message::user("hello")],
            vec![],
            Some(tx),
            Some(token),
        ),
    )
    .await
    .expect("cancel without waiting for backoff")
    .expect("cancelled run");
    watcher.await.expect("watcher");
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}
