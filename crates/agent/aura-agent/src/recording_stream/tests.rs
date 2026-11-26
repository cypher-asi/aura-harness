use super::*;
use aura_agent_kernel::{ExecutorRouter, Kernel, KernelConfig};
use aura_core_types::{AgentId, TransactionType};
use aura_model::{Message, MockProvider, ModelProvider, ModelRequest, StreamEvent};
use aura_store_db::{RocksStore, Store};
use futures_util::{stream, StreamExt};
use std::sync::Arc;
use tempfile::TempDir;

struct TwoEventProvider;

#[async_trait::async_trait]
impl ModelProvider for TwoEventProvider {
    fn name(&self) -> &'static str {
        "two-event-mock"
    }

    async fn complete(
        &self,
        _request: ModelRequest,
    ) -> Result<aura_model::ModelResponse, ReasonerError> {
        Err(ReasonerError::Internal("streaming-only mock".into()))
    }

    async fn complete_streaming(
        &self,
        _request: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        let events: Vec<Result<StreamEvent, ReasonerError>> = vec![
            Ok(StreamEvent::MessageStart {
                message_id: "msg-1".to_string(),
                model: "mock-stream-model".to_string(),
                input_tokens: Some(3),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
            Ok(StreamEvent::TextDelta {
                text: "partial".to_string(),
            }),
        ];
        Ok(Box::pin(stream::iter(events)))
    }

    async fn health_check(&self) -> bool {
        true
    }
}

fn create_kernel(provider: Arc<dyn ModelProvider + Send + Sync>) -> (Arc<Kernel>, TempDir) {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let executor = ExecutorRouter::new();
    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        ..KernelConfig::default()
    };
    let kernel = Arc::new(Kernel::new(store, provider, executor, config, agent_id).unwrap());
    (kernel, db_dir)
}

// Provider that yields two events, then a transport-level
// `Err(ReasonerError)`. Exercises the mid-stream error finalization
// path (Invariant §3 / T3.a).
struct ErrorMidStreamProvider;

#[async_trait::async_trait]
impl ModelProvider for ErrorMidStreamProvider {
    fn name(&self) -> &'static str {
        "error-midstream-mock"
    }

    async fn complete(
        &self,
        _request: ModelRequest,
    ) -> Result<aura_model::ModelResponse, ReasonerError> {
        Err(ReasonerError::Internal("streaming-only mock".into()))
    }

    async fn complete_streaming(
        &self,
        _request: ModelRequest,
    ) -> Result<aura_model::StreamEventStream, ReasonerError> {
        let events: Vec<Result<StreamEvent, ReasonerError>> = vec![
            Ok(StreamEvent::MessageStart {
                message_id: "msg-err-1".to_string(),
                model: "mock-stream-model".to_string(),
                input_tokens: Some(5),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
            Ok(StreamEvent::TextDelta {
                text: "partial".to_string(),
            }),
            Err(ReasonerError::Internal(
                "simulated mid-stream failure".into(),
            )),
        ];
        Ok(Box::pin(stream::iter(events)))
    }

    async fn health_check(&self) -> bool {
        true
    }
}

// ----------------------------------------------------------------
// Invariant §3 enforcement — streaming recording tests.
//
// The three tests below pin Invariant §3 ("every LLM call is
// recorded") against each terminal path of `RecordingStream`:
//
//   - `streaming_natural_end_records_completed`: `MessageStop` /
//     end-of-stream → exactly one Reasoning entry, not failed.
//   - `streaming_error_records_failed`: a mid-stream
//     `Err(ReasonerError)` → exactly one failed Reasoning entry,
//     no second entry from the `Drop` path.
//   - `streaming_drop_records_failed`: early `drop()` before any
//     terminal event → exactly one failed entry with
//     `STREAM_DROPPED_REASON`.
// ----------------------------------------------------------------

#[tokio::test]
async fn streaming_drop_records_failed() {
    let provider: Arc<dyn ModelProvider + Send + Sync> = Arc::new(TwoEventProvider);
    let (kernel, _db) = create_kernel(provider);

    let request = ModelRequest::builder("mock-stream-model", "system")
        .message(Message::user("hi"))
        .try_build()
        .unwrap();
    let (handle, inner) = kernel.reason_streaming(request).await.unwrap();
    let mut stream = RecordingStream::new(inner, handle);

    // Consume only the first event, then drop the stream.
    let first = stream.next().await;
    assert!(first.is_some(), "expected at least one streamed event");
    drop(stream);

    // The kernel's store should now contain exactly one Reasoning
    // entry marked as failed with the mid-stream drop reason.
    let entries = kernel
        .store()
        .scan_record(kernel.agent_id, 0, 64)
        .expect("scan_record");
    let reasoning: Vec<_> = entries
        .iter()
        .filter(|e| e.tx.tx_type == TransactionType::Reasoning)
        .collect();
    assert_eq!(
        reasoning.len(),
        1,
        "expected exactly one reasoning entry, got {}",
        reasoning.len()
    );

    let payload: serde_json::Value =
        serde_json::from_slice(&reasoning[0].tx.payload).expect("reasoning payload is json");
    assert_eq!(
        payload.get("status").and_then(|v| v.as_str()),
        Some("failed")
    );
    assert_eq!(
        payload.get("error").and_then(|v| v.as_str()),
        Some(STREAM_DROPPED_REASON)
    );
}

#[tokio::test]
async fn streaming_natural_end_records_completed() {
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("hello world"));
    let (kernel, _db) = create_kernel(provider);

    let request = ModelRequest::builder("test-model", "system")
        .message(Message::user("hi"))
        .try_build()
        .unwrap();
    let (handle, inner) = kernel.reason_streaming(request).await.unwrap();
    let mut stream = RecordingStream::new(inner, handle);
    while stream.next().await.is_some() {}
    drop(stream);

    let entries = kernel
        .store()
        .scan_record(kernel.agent_id, 0, 64)
        .expect("scan_record");
    let reasoning: Vec<_> = entries
        .iter()
        .filter(|e| e.tx.tx_type == TransactionType::Reasoning)
        .collect();
    assert_eq!(reasoning.len(), 1);

    let payload: serde_json::Value =
        serde_json::from_slice(&reasoning[0].tx.payload).expect("reasoning payload is json");
    assert!(
        payload.get("status").is_none(),
        "natural end must not tag the entry as failed: {payload}"
    );
    assert!(payload.get("stop_reason").is_some());
}

#[tokio::test]
async fn streaming_error_records_failed() {
    let provider: Arc<dyn ModelProvider + Send + Sync> = Arc::new(ErrorMidStreamProvider);
    let (kernel, _db) = create_kernel(provider);

    let request = ModelRequest::builder("mock-stream-model", "system")
        .message(Message::user("hi"))
        .try_build()
        .unwrap();
    let (handle, inner) = kernel.reason_streaming(request).await.unwrap();
    let mut stream = RecordingStream::new(inner, handle);

    // Drain the stream fully. The third item is `Err(_)` which
    // must trigger the failed-finalization path. After drain the
    // stream is exhausted and `drop()` must observe `handle ==
    // None` (otherwise it would append a second failure entry —
    // see the double-finalization assertion below).
    let mut saw_error = false;
    while let Some(item) = stream.next().await {
        if item.is_err() {
            saw_error = true;
        }
    }
    assert!(saw_error, "mock stream must yield exactly one error item");
    drop(stream);

    let entries = kernel
        .store()
        .scan_record(kernel.agent_id, 0, 64)
        .expect("scan_record");
    let reasoning: Vec<_> = entries
        .iter()
        .filter(|e| e.tx.tx_type == TransactionType::Reasoning)
        .collect();

    // Exactly one reasoning entry — the `Drop` finalizer must be a
    // no-op after an explicit error finalization.
    assert_eq!(
        reasoning.len(),
        1,
        "expected exactly one reasoning entry after mid-stream error, got {}: {:?}",
        reasoning.len(),
        reasoning.iter().map(|e| &e.tx.payload).collect::<Vec<_>>(),
    );

    let payload: serde_json::Value =
        serde_json::from_slice(&reasoning[0].tx.payload).expect("reasoning payload is json");
    assert_eq!(
        payload.get("status").and_then(|v| v.as_str()),
        Some("failed"),
        "expected status=failed, got: {payload}"
    );
    let err_text = payload
        .get("error")
        .and_then(|v| v.as_str())
        .expect("error field present");
    assert!(
        err_text.contains("simulated mid-stream failure"),
        "expected mid-stream error text, got: {err_text}"
    );
    assert_ne!(
        err_text, STREAM_DROPPED_REASON,
        "mid-stream error must not be attributed to the Drop path"
    );
}

struct ThinkingProvider;

#[async_trait::async_trait]
impl ModelProvider for ThinkingProvider {
    fn name(&self) -> &'static str {
        "thinking-mock"
    }

    async fn complete(
        &self,
        _request: ModelRequest,
    ) -> Result<aura_model::ModelResponse, ReasonerError> {
        Err(ReasonerError::Internal("streaming-only mock".into()))
    }

    async fn complete_streaming(
        &self,
        _request: ModelRequest,
    ) -> Result<aura_model::StreamEventStream, ReasonerError> {
        let events: Vec<Result<StreamEvent, ReasonerError>> = vec![
            Ok(StreamEvent::MessageStart {
                message_id: "msg-think-1".to_string(),
                model: "mock-stream-model".to_string(),
                input_tokens: Some(3),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
            Ok(StreamEvent::ThinkingDelta {
                thinking: "Let me ".to_string(),
            }),
            Ok(StreamEvent::ThinkingDelta {
                thinking: "check.".to_string(),
            }),
            Ok(StreamEvent::TextDelta {
                text: "Done".to_string(),
            }),
            Ok(StreamEvent::MessageStop),
        ];
        Ok(Box::pin(stream::iter(events)))
    }

    async fn health_check(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn streaming_records_thinking_text() {
    let provider: Arc<dyn ModelProvider + Send + Sync> = Arc::new(ThinkingProvider);
    let (kernel, _db) = create_kernel(provider);

    let request = ModelRequest::builder("mock-stream-model", "system")
        .message(Message::user("hi"))
        .try_build()
        .unwrap();
    let (handle, inner) = kernel.reason_streaming(request).await.unwrap();
    let mut stream = RecordingStream::new(inner, handle);
    while stream.next().await.is_some() {}
    drop(stream);

    let entries = kernel
        .store()
        .scan_record(kernel.agent_id, 0, 64)
        .expect("scan_record");
    let reasoning = entries
        .iter()
        .find(|e| e.tx.tx_type == TransactionType::Reasoning)
        .expect("reasoning entry");
    let payload: serde_json::Value =
        serde_json::from_slice(&reasoning.tx.payload).expect("reasoning payload is json");
    assert_eq!(
        payload.get("thinking").and_then(|v| v.as_str()),
        Some("Let me check.")
    );
    assert!(payload.get("thinking_ms").is_some(), "{payload}");
}

#[tokio::test]
async fn plan_usage_is_recorded_on_streamed_and_complete_calls() {
    use aura_model::{Plan, PlanUsage};
    let reading = PlanUsage {
        plan: Plan::Claude,
        window: "5h".to_string(),
        used: 0.25,
        resets_at: Some(1_764_554_400),
    };
    let provider: Arc<dyn ModelProvider + Send + Sync> = Arc::new(
        MockProvider::new()
            .with_default_response(aura_model::MockResponse::text("ok"))
            .with_plan_usage(vec![reading.clone()]),
    );
    let (kernel, _db) = create_kernel(provider);
    let request = || {
        ModelRequest::builder("claude/claude-sonnet-4-6", "system")
            .message(Message::user("hi"))
            .try_build()
            .unwrap()
    };

    let (handle, inner) = kernel.reason_streaming(request()).await.unwrap();
    let mut stream = RecordingStream::new(inner, handle);
    while stream.next().await.is_some() {}
    drop(stream);
    let result = kernel.reason(request()).await.unwrap();
    assert_eq!(
        result.response.trace.plan_usage,
        std::slice::from_ref(&reading)
    );

    let entries = kernel
        .store()
        .scan_record(kernel.agent_id, 0, 64)
        .expect("scan_record");
    let recorded: Vec<Vec<PlanUsage>> = entries
        .iter()
        .filter(|e| e.tx.tx_type == TransactionType::Reasoning)
        .map(|e| {
            let payload: serde_json::Value = serde_json::from_slice(&e.tx.payload).unwrap();
            serde_json::from_value(payload["plan_usage"].clone()).unwrap()
        })
        .collect();
    assert_eq!(recorded, [vec![reading.clone()], vec![reading]]);
}

#[tokio::test]
async fn calls_without_plan_usage_record_none() {
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("hello"));
    let (kernel, _db) = create_kernel(provider);
    let request = ModelRequest::builder("test-model", "system")
        .message(Message::user("hi"))
        .try_build()
        .unwrap();
    let (handle, inner) = kernel.reason_streaming(request).await.unwrap();
    let mut stream = RecordingStream::new(inner, handle);
    while stream.next().await.is_some() {}
    drop(stream);
    let entries = kernel
        .store()
        .scan_record(kernel.agent_id, 0, 64)
        .expect("scan_record");
    let reasoning = entries
        .iter()
        .find(|e| e.tx.tx_type == TransactionType::Reasoning)
        .expect("reasoning entry");
    let payload: serde_json::Value = serde_json::from_slice(&reasoning.tx.payload).unwrap();
    assert!(payload.get("plan_usage").is_none(), "{payload}");
}
