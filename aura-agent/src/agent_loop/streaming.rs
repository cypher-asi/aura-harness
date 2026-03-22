//! Streaming model calls and event emission.

use std::time::Instant;

use aura_reasoner::{
    ModelProvider, ModelRequest, ModelResponse, StreamAccumulator, StreamContentType, StreamEvent,
};
use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::events::AgentLoopEvent;

use super::AgentLoop;

/// Send an event through the channel if present.
pub(super) fn emit(tx: &Option<UnboundedSender<AgentLoopEvent>>, event: AgentLoopEvent) {
    if let Some(tx) = tx {
        let _ = tx.send(event);
    }
}

/// Emit an [`AgentLoopEvent::IterationComplete`] event.
pub(super) fn emit_iteration_complete(
    event_tx: &Option<UnboundedSender<AgentLoopEvent>>,
    iteration: usize,
    response: &ModelResponse,
) {
    emit(
        event_tx,
        AgentLoopEvent::IterationComplete {
            iteration,
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
        },
    );
}

/// Map a [`StreamEvent`] to the corresponding [`AgentLoopEvent`] and emit it.
fn emit_stream_event(
    event_tx: &Option<UnboundedSender<AgentLoopEvent>>,
    stream_event: &StreamEvent,
    accumulator: &StreamAccumulator,
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
            if let Some(ref tool) = accumulator.current_tool_use {
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
        StreamEvent::Error { message } => {
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

impl AgentLoop {
    /// Perform a model completion using streaming, emitting events as they arrive.
    ///
    /// Falls back to non-streaming `provider.complete()` if the streaming call
    /// itself returns an error.
    #[allow(clippy::cast_possible_truncation)]
    pub(super) async fn complete_with_streaming(
        &self,
        provider: &dyn ModelProvider,
        request: ModelRequest,
        event_tx: &Option<UnboundedSender<AgentLoopEvent>>,
        cancellation_token: Option<&CancellationToken>,
    ) -> anyhow::Result<ModelResponse> {
        let start = Instant::now();

        match provider.complete_streaming(request.clone()).await {
            Ok(mut stream) => {
                let mut accumulator = StreamAccumulator::new();

                loop {
                    let next = if let Some(token) = cancellation_token {
                        tokio::select! {
                            _ = token.cancelled() => {
                                return Err(anyhow::anyhow!("Cancelled"));
                            }
                            item = stream.next() => item,
                        }
                    } else {
                        stream.next().await
                    };

                    match next {
                        Some(Ok(event)) => {
                            accumulator.process(&event);
                            emit_stream_event(event_tx, &event, &accumulator);
                        }
                        Some(Err(e)) => {
                            debug!("Stream error, falling back to non-streaming: {e}");
                            emit(
                                event_tx,
                                AgentLoopEvent::Warning(format!(
                                    "Stream error, retrying without streaming: {e}"
                                )),
                            );
                            return provider.complete(request).await;
                        }
                        None => break,
                    }
                }

                let latency_ms = start.elapsed().as_millis() as u64;
                accumulator.into_response(0, latency_ms)
            }
            Err(e) => {
                debug!("complete_streaming failed, falling back: {e}");
                emit(
                    event_tx,
                    AgentLoopEvent::Warning(format!(
                        "Streaming unavailable, using non-streaming: {e}"
                    )),
                );
                provider.complete(request).await
            }
        }
    }
}
