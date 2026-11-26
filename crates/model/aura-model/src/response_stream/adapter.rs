//! Adapter lifting the per-delta [`StreamEventStream`] into the
//! completed-item [`ResponseEventStream`] surface.

use std::collections::VecDeque;

use futures_util::StreamExt;
use serde_json::Value;

use super::{OutputItem, ResponseEvent, ResponseEventStream, StreamError, StreamPhase};
use crate::types::{
    PartialToolUse, StopReason, StreamAccumulator, StreamContentType, StreamEvent, Usage,
};
use crate::StreamEventStream;

/// Lift a low-level [`StreamEventStream`] into a [`ResponseEventStream`].
///
/// Aggregates per-delta wire frames into completed items: text /
/// thinking deltas accumulate into a single block until the matching
/// `ContentBlockStop` fires, at which point an
/// [`OutputItem`] is emitted. The terminal `MessageStop` is converted
/// into a [`ResponseEvent::Completed`] with the accumulator's
/// observed stop reason and usage counters.
///
/// Transport errors surface as `Some(Err(StreamError::TransportClosed))`
/// at the point they arrive. Invalid completed tool JSON produces an
/// [`OutputItem::InvalidToolUse`], allowing a paired error result without
/// executing malformed arguments or discarding the terminal stop reason.
#[must_use]
pub fn response_stream_from_event_stream(stream: StreamEventStream) -> ResponseEventStream {
    let state = AdapterState {
        stream,
        accumulator: StreamAccumulator::new(),
        pending: VecDeque::new(),
        completed_emitted: false,
        terminated: false,
    };
    Box::pin(futures_util::stream::unfold(
        state,
        |mut state| async move {
            if state.terminated {
                return None;
            }
            loop {
                if let Some(event) = state.pending.pop_front() {
                    return Some((event, state));
                }

                match state.stream.next().await {
                    Some(Ok(event)) => {
                        state.accumulator.process(&event);
                        state.handle_event(&event);
                    }
                    Some(Err(err)) => {
                        state.terminated = true;
                        let err = state.transport_error(err.to_string());
                        return Some((Err(err), state));
                    }
                    None => {
                        state.terminated = true;
                        if let Some(err) = state.closed_with_partial_error() {
                            return Some((Err(err), state));
                        }
                        if !state.completed_emitted {
                            return Some((
                                Err(StreamError::TransportClosed {
                                    context: "missing message_stop".into(),
                                }),
                                state,
                            ));
                        }
                        return None;
                    }
                }
            }
        },
    ))
}

struct AdapterState {
    stream: StreamEventStream,
    accumulator: StreamAccumulator,
    pending: VecDeque<Result<ResponseEvent, StreamError>>,
    completed_emitted: bool,
    terminated: bool,
}

impl AdapterState {
    fn handle_event(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::ContentBlockStop { .. } => {
                if let Some(item) = self.take_completed_item() {
                    self.pending
                        .push_back(Ok(ResponseEvent::OutputItemDone(item)));
                }
            }
            StreamEvent::MessageStop => {
                let event = self.synthesize_completed();
                self.pending.push_back(Ok(event));
            }
            StreamEvent::Error { message, .. } => {
                let err = self.provider_error(message.clone());
                match err {
                    StreamError::StreamAbortedWithPartial { .. } => {
                        self.pending.push_back(Err(err));
                    }
                    other => {
                        self.pending.push_back(Ok(ResponseEvent::Error(other)));
                    }
                }
            }
            _ => {
                if let Some(progress) = progress_event(event) {
                    self.pending.push_back(Ok(progress));
                }
            }
        }
    }

    fn provider_error(&mut self, message: String) -> StreamError {
        if self.accumulator.current_tool_use.is_some() {
            return StreamError::StreamAbortedWithPartial {
                reason: self.accumulator.stream_error_reason(&message),
                partial_tool_use: self.take_partial_tool_use(),
            };
        }
        StreamError::InvalidEvent { reason: message }
    }

    fn transport_error(&mut self, context: String) -> StreamError {
        if self.accumulator.current_tool_use.is_some() {
            return StreamError::StreamAbortedWithPartial {
                reason: self.accumulator.stream_error_reason(&context),
                partial_tool_use: self.take_partial_tool_use(),
            };
        }
        StreamError::TransportClosed { context }
    }

    fn closed_with_partial_error(&mut self) -> Option<StreamError> {
        if self.accumulator.current_tool_use.is_some() {
            Some(StreamError::StreamAbortedWithPartial {
                reason: self
                    .accumulator
                    .stream_error_reason("transport closed before stream completed"),
                partial_tool_use: self.take_partial_tool_use(),
            })
        } else {
            None
        }
    }

    fn take_partial_tool_use(&mut self) -> Option<PartialToolUse> {
        self.accumulator
            .current_tool_use
            .take()
            .map(|pending| PartialToolUse {
                tool_use_id: pending.id,
                tool_name: pending.name,
                partial_json: pending.input_json,
            })
    }

    /// Pull whichever item the accumulator finished last. The
    /// accumulator's `process(ContentBlockStop)` pushed the closing
    /// tool_use onto `tool_uses`; text / thinking blocks live in the
    /// running accumulators instead.
    fn take_completed_item(&mut self) -> Option<OutputItem> {
        if let Some(tool) = self.accumulator.tool_uses.pop() {
            let input: Value = if tool.input_json.is_empty() {
                serde_json::json!({})
            } else {
                match serde_json::from_str(&tool.input_json) {
                    Ok(input) => input,
                    Err(err) => return Some(OutputItem::InvalidToolUse {
                        id: tool.id, name: tool.name,
                        reason: format!("Tool arguments were incomplete or invalid JSON: {err}. No tool was executed; regenerate this call with complete arguments."),
                    }),
                }
            };
            return Some(OutputItem::ToolUse {
                id: tool.id,
                name: tool.name,
                input,
            });
        }
        if !self.accumulator.thinking_content.is_empty() {
            let thinking = std::mem::take(&mut self.accumulator.thinking_content);
            let signature = self.accumulator.thinking_signature.take();
            return Some(OutputItem::Thinking {
                thinking,
                signature,
            });
        }
        if !self.accumulator.text_content.is_empty() {
            let text = std::mem::take(&mut self.accumulator.text_content);
            return Some(OutputItem::Message { text });
        }
        None
    }

    fn synthesize_completed(&mut self) -> ResponseEvent {
        self.completed_emitted = true;
        let end_turn = self
            .accumulator
            .stop_reason
            .map(|reason| matches!(reason, StopReason::EndTurn | StopReason::StopSequence));
        let usage = Usage {
            input_tokens: self.accumulator.input_tokens,
            output_tokens: self.accumulator.output_tokens,
            cache_creation_input_tokens: self.accumulator.cache_creation_input_tokens,
            cache_read_input_tokens: self.accumulator.cache_read_input_tokens,
        };
        ResponseEvent::Completed {
            end_turn,
            stop_reason: self.accumulator.stop_reason,
            usage,
        }
    }
}

/// The liveness / incremental event for an intra-block delta or keepalive
/// frame; `None` for the frames [`AdapterState::handle_event`] handles itself.
fn progress_event(event: &StreamEvent) -> Option<ResponseEvent> {
    let progress = match event {
        StreamEvent::ContentBlockStop { .. }
        | StreamEvent::MessageStop
        | StreamEvent::Error { .. } => return None,
        // Every other frame is an intra-block delta or a keepalive
        // ping. These used to be silently swallowed by the unfold
        // loop, which meant a consumer's per-event timeout only saw
        // *completed* blocks and could not tell a slow-but-alive
        // stream from a dead one. We now surface each as a
        // phase-tagged [`ResponseEvent::Keepalive`] so the consumer
        // can reset its liveness timer and log the streaming phase.
        StreamEvent::Ping => ResponseEvent::Keepalive(StreamPhase::Ping),
        StreamEvent::MessageStart { .. }
        | StreamEvent::HttpMeta { .. }
        | StreamEvent::MessageDelta { .. } => ResponseEvent::Keepalive(StreamPhase::Connecting),
        StreamEvent::ContentBlockStart { content_type, .. } => match content_type {
            StreamContentType::Thinking => ResponseEvent::Keepalive(StreamPhase::Thinking),
            StreamContentType::Text => ResponseEvent::Keepalive(StreamPhase::Text),
            StreamContentType::ToolUse { id, name } => ResponseEvent::OutputToolUseStart {
                id: id.clone(),
                name: name.clone(),
            },
        },
        // Carry the actual thinking text through so the pump can
        // stream it incrementally. The chunk also serves as a
        // liveness signal (any yielded item resets the per-event
        // timeout). Signature deltas carry no user-visible text, so
        // they stay a content-free thinking keepalive.
        StreamEvent::ThinkingDelta { thinking } => {
            ResponseEvent::OutputThinkingDelta(thinking.clone())
        }
        StreamEvent::SignatureDelta { .. } => ResponseEvent::Keepalive(StreamPhase::Thinking),
        // Carry the actual assistant text through for incremental
        // streaming (same liveness rationale as thinking deltas).
        StreamEvent::TextDelta { text } => ResponseEvent::OutputTextDelta(text.clone()),
        StreamEvent::InputJsonDelta { partial_json } => {
            ResponseEvent::OutputToolInputDelta(partial_json.clone())
        }
    };
    Some(progress)
}
