//! Model-call capture for run traces.
//!
//! Builds the [`ModelExchange`] emitted after every sampling request.
//! When the loop has a blob store, the exact request (system prompt,
//! tool schemas, every message) and the response's message and
//! thinking text are written as content-addressed blobs so the call
//! can be replayed or turned into a training example later. Without a
//! store only the action summary (thinking text included) and usage are
//! recorded.

use std::sync::Arc;

use aura_core_types::trace::{
    blob_id, BlobId, ModelAction, ModelExchange, ModelObservation, TokenUsage, ToolCallAction,
    ToolIntent,
};
use aura_model::{ContentBlock, ModelRequest, ModelResponse};
use aura_store_snapshot::SnapshotStore;
use chrono::Utc;
use serde_json::json;
use tokio::sync::mpsc::Sender;

use crate::events::{AgentLoopEvent, DebugEvent};

use super::streaming;

/// Write `bytes` to `store` and return its address. Store failures
/// are logged and the address is still returned so the trace keeps a
/// stable reference.
fn put(store: &dyn SnapshotStore, bytes: Vec<u8>) -> BlobId {
    let id = blob_id(&bytes);
    if let Err(e) = store.put(&id, bytes.into()) {
        tracing::warn!(error = %e, "trace blob write failed");
    }
    id
}

fn put_json<T: serde::Serialize + ?Sized>(store: &dyn SnapshotStore, value: &T) -> BlobId {
    put(store, serde_json::to_vec(value).unwrap_or_default())
}

/// Capture what the model is about to see. Credentials are never part
/// of the captured shape.
pub(super) fn observe(
    request: &ModelRequest,
    store: Option<&Arc<dyn SnapshotStore>>,
) -> Option<ModelObservation> {
    let store = store?.as_ref();
    Some(ModelObservation {
        system: put(store, request.system.clone().into_bytes()),
        tools: put_json(store, &request.tools),
        messages: request
            .messages
            .iter()
            .map(|m| put_json(store, m))
            .collect(),
        params: json!({
            "model": request.model.to_string(),
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "thinking": request.thinking,
            "thinking_effort": request.thinking_effort.map(|e| format!("{e:?}").to_lowercase()),
            "tool_choice": request.tool_choice,
            "tool_count": request.tools.len(),
        }),
    })
}

/// Summarize what the model produced.
pub(super) fn action(
    response: &ModelResponse,
    store: Option<&Arc<dyn SnapshotStore>>,
) -> ModelAction {
    let mut text = String::new();
    let mut thinking = String::new();
    let mut tool_calls = Vec::new();
    for block in &response.message.content {
        match block {
            ContentBlock::Text { text: t } => text.push_str(t),
            ContentBlock::Thinking { thinking: t, .. } => thinking.push_str(t),
            ContentBlock::ToolUse { id, name, input } => tool_calls.push(ToolCallAction {
                tool_use_id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                intent: ToolIntent::classify(name, input),
            }),
            _ => {}
        }
    }
    let store = store.map(AsRef::as_ref);
    ModelAction {
        message: store.map(|s| put_json(s, &response.message)),
        thinking: store
            .filter(|_| !thinking.is_empty())
            .map(|s| put(s, thinking.clone().into_bytes())),
        thinking_chars: thinking.chars().count(),
        thinking_text: thinking,
        text,
        tool_calls,
        stop_reason: serde_json::to_value(response.stop_reason)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default(),
        purpose: None,
        error: None,
    }
}

/// Emit the exchange for one completed sampling request.
pub(super) fn emit(
    event_tx: Option<&Sender<AgentLoopEvent>>,
    iteration: usize,
    model: &str,
    observation: Option<ModelObservation>,
    response: &ModelResponse,
    store: Option<&Arc<dyn SnapshotStore>>,
    latency_ms: u64,
) {
    if event_tx.is_none() {
        return;
    }
    let usage = &response.usage;
    let exchange = ModelExchange {
        iteration: u32::try_from(iteration).unwrap_or(u32::MAX),
        model: if response.trace.model.is_empty() {
            model.to_string()
        } else {
            response.trace.model.clone()
        },
        observation,
        action: action(response, store),
        usage: TokenUsage {
            input: usage.input_tokens,
            output: usage.output_tokens,
            cache_write: usage.cache_creation_input_tokens.unwrap_or_default(),
            cache_read: usage.cache_read_input_tokens.unwrap_or_default(),
        },
        latency_ms,
        provider_request_id: response.trace.provider_request_id.clone(),
    };
    streaming::emit(
        event_tx,
        AgentLoopEvent::Debug(DebugEvent::ModelExchange {
            timestamp: Utc::now(),
            exchange: Box::new(exchange),
            task_id: None,
        }),
    );
}
