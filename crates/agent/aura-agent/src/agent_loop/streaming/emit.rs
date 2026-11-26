//! Event emission helpers for the streaming model-call path.

use std::time::Instant;

use aura_model::{ModelResponse, StreamAccumulator, StreamContentType, StreamEvent};
use chrono::Utc;
use tokio::sync::mpsc::Sender;

use crate::agent_loop::event_sink;
use crate::events::{AgentLoopEvent, DebugEvent};

/// Send an event through the channel if present.
///
/// Phase 4: thin re-export of [`event_sink::emit`] so legacy
/// `streaming::emit(...)` call sites keep compiling while the
/// canonical entry point lives next to the unified sink policy.
/// The previous local `try_send + warn` body and the blocking
/// `emit_with_backpressure` helper were removed during the dual-path
/// collapse (see [`event_sink`] module docs for the policy).
pub(in crate::agent_loop) fn emit(tx: Option<&Sender<AgentLoopEvent>>, event: AgentLoopEvent) {
    event_sink::emit(tx, event);
}

/// Emit an [`AgentLoopEvent::IterationComplete`] event along with the
/// matching [`DebugEvent::Iteration`] frame for the `aura-os` run
/// bundle. `duration_ms` reflects wall-clock time since the start of
/// the current iteration (model call + tool dispatch); `tool_calls` is
/// the number of `ContentBlock::ToolUse` blocks in the response.
pub(in crate::agent_loop) fn emit_iteration_complete(
    event_tx: Option<&Sender<AgentLoopEvent>>,
    iteration: usize,
    response: &ModelResponse,
    iteration_started_at: Instant,
) {
    emit(
        event_tx,
        AgentLoopEvent::IterationComplete {
            iteration,
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
        },
    );

    let tool_calls = response
        .message
        .content
        .iter()
        .filter(|b| matches!(b, aura_model::ContentBlock::ToolUse { .. }))
        .count();

    let duration_ms = u64::try_from(iteration_started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
    let index = u32::try_from(iteration).unwrap_or(u32::MAX);
    let tool_calls = u32::try_from(tool_calls).unwrap_or(u32::MAX);

    emit(
        event_tx,
        AgentLoopEvent::Debug(DebugEvent::Iteration {
            timestamp: Utc::now(),
            index,
            tool_calls,
            duration_ms,
            task_id: None,
        }),
    );
}

/// Emit a [`DebugEvent::LlmCall`] frame. Called at the end of every
/// completed provider call (streaming happy path, non-streaming
/// fallback path, the compact-and-retry path, and the stream pump).
pub(in crate::agent_loop) fn emit_debug_llm_call(
    event_tx: Option<&Sender<AgentLoopEvent>>,
    provider_name: &str,
    model_name: &str,
    response: &ModelResponse,
    duration_ms: u64,
) {
    let model = if response.trace.model.is_empty() {
        model_name.to_string()
    } else {
        response.trace.model.clone()
    };
    emit(
        event_tx,
        AgentLoopEvent::Debug(DebugEvent::LlmCall {
            timestamp: Utc::now(),
            provider: provider_name.to_string(),
            model,
            route: Some(model_name.to_string()),
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
            cache_creation_input_tokens: response.usage.cache_creation_input_tokens,
            cache_read_input_tokens: response.usage.cache_read_input_tokens,
            duration_ms,
            task_id: None,
            agent_instance_id: None,
            provider_request_id: response.trace.provider_request_id.clone(),
            message_id: response.trace.message_id.clone(),
        }),
    );
}

/// Map a [`StreamEvent`] to the corresponding [`AgentLoopEvent`] and emit it.
///
/// Phase 4: routes through the unified [`event_sink::emit`] policy
/// (`try_send` + warn/debug) so the buffered streaming path no longer
/// awaits on a saturated channel. The previous
/// `emit_with_backpressure` helper that backpressured the loop on a
/// slow forwarder is gone; per-delta drops are now logged but do not
/// stall sampling.
pub(super) fn emit_stream_event(
    event_tx: Option<&Sender<AgentLoopEvent>>,
    stream_event: &StreamEvent,
    accumulator: &StreamAccumulator,
    last_snapshot: &mut Option<Instant>,
) {
    if event_tx.is_none() {
        return;
    }

    match stream_event {
        StreamEvent::TextDelta { text } => {
            emit(event_tx, AgentLoopEvent::TextDelta(text.clone()));
        }
        StreamEvent::ThinkingDelta { thinking } => {
            emit(event_tx, AgentLoopEvent::ThinkingDelta(thinking.clone()));
        }
        StreamEvent::ContentBlockStart {
            content_type: StreamContentType::ToolUse { id, name },
            ..
        } => {
            emit(
                event_tx,
                AgentLoopEvent::ToolStart {
                    id: id.clone(),
                    name: name.clone(),
                },
            );
        }
        StreamEvent::InputJsonDelta { .. } => {
            let due = last_snapshot.is_none_or(|at| {
                at.elapsed() >= crate::agent_loop::stream_pump::TOOL_INPUT_SNAPSHOT_INTERVAL
            });
            if let Some(tool) = accumulator.current_tool_use.as_ref().filter(|_| due) {
                *last_snapshot = Some(Instant::now());
                emit(
                    event_tx,
                    AgentLoopEvent::ToolInputSnapshot {
                        id: tool.id.clone(),
                        name: tool.name.clone(),
                        input: tool.input_json.clone(),
                    },
                );
            }
        }
        StreamEvent::Error { message, .. } => {
            emit(
                event_tx,
                AgentLoopEvent::Error {
                    code: "stream_error".to_string(),
                    message: message.clone(),
                    recoverable: true,
                },
            );
        }
        _ => {}
    }
}
