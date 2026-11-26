//! Per-event handlers of [`StreamDrive`]: one method per streamed
//! [`ResponseEvent`] kind.

use std::ops::ControlFlow;

use aura_model::{OutputItem, ResponseEvent, StopReason, StreamPhase};

use crate::agent_loop::LoopState;
use crate::events::AgentLoopEvent;
use crate::types::ToolCallInfo;

use super::super::{emit_event, StreamPumpOutcome};
use super::{OpenToolCall, StreamDrive, TOOL_INPUT_SNAPSHOT_INTERVAL};

impl StreamDrive<'_> {
    /// Fold one stream event into the loop state; `Break` ends the
    /// event loop with the stop reason or a short-circuit outcome.
    pub(super) async fn on_event(
        &mut self,
        event: ResponseEvent,
        state: &mut LoopState,
    ) -> ControlFlow<Result<Option<StopReason>, StreamPumpOutcome>> {
        match event {
            ResponseEvent::OutputItemDone(OutputItem::InvalidToolUse { id, name, reason }) => {
                self.on_invalid_tool_use(id, name, reason);
            }
            ResponseEvent::OutputItemDone(OutputItem::ToolUse { id, name, input }) => {
                if let Some(err) = self
                    .on_tool_use(ToolCallInfo { id, name, input }, state)
                    .await
                {
                    return ControlFlow::Break(Err(err));
                }
            }
            ResponseEvent::OutputTextDelta(chunk) => self.on_text_delta(chunk),
            ResponseEvent::OutputThinkingDelta(chunk) => self.on_thinking_delta(chunk),
            ResponseEvent::OutputItemDone(OutputItem::Message { text }) => {
                self.on_message_done(text);
            }
            ResponseEvent::OutputItemDone(OutputItem::Thinking {
                thinking,
                signature,
            }) => self.on_thinking_done(thinking, signature),
            ResponseEvent::Completed {
                end_turn,
                stop_reason,
                usage,
            } => {
                self.end_turn = end_turn;
                self.usage = usage;
                return ControlFlow::Break(Ok(stop_reason));
            }
            ResponseEvent::Error(err) => return ControlFlow::Break(Err(self.interrupted(err))),
            ResponseEvent::OutputToolUseStart { id, name } => self.on_tool_use_start(id, name),
            ResponseEvent::OutputToolInputDelta(chunk) => self.on_tool_input_delta(&chunk),
            ResponseEvent::Keepalive(phase) => {
                // A ping or intra-block delta. Reaching this arm
                // already reset the per-event liveness timeout
                // (the stream yielded an item, so the loop
                // re-enters `next_stream_step` with a fresh
                // window).
                self.liveness.beat(phase, self.ctx.event_tx);
            }
        }
        ControlFlow::Continue(())
    }

    fn on_text_delta(&mut self, chunk: String) {
        self.text_bytes += chunk.len();
        // Incremental assistant text: stream it to the
        // client the moment it arrives so a long writing
        // phase is never a silent gap. The full block is
        // still accumulated at `OutputItemDone(Message)`
        // for final-response synthesis.
        self.streamed_text_in_block = true;
        emit_event(self.ctx.event_tx, AgentLoopEvent::TextDelta(chunk));
    }

    fn on_thinking_delta(&mut self, chunk: String) {
        self.thinking_bytes += chunk.len();
        // Incremental extended-thinking: stream it live so
        // the client can render the current thinking block
        // instead of waiting out a multi-second think.
        self.streamed_thinking_in_block = true;
        emit_event(self.ctx.event_tx, AgentLoopEvent::ThinkingDelta(chunk));
    }

    fn on_message_done(&mut self, text: String) {
        // Block close. Only emit the whole block as a delta
        // when nothing was streamed incrementally for it
        // (the buffered/synthesised fallback stream, which
        // emits no `OutputTextDelta`); otherwise the client
        // already has the text and a re-emit would double
        // it. Always accumulate for `synthesize_response`.
        if !self.streamed_text_in_block {
            self.text_bytes += text.len();
            emit_event(self.ctx.event_tx, AgentLoopEvent::TextDelta(text.clone()));
        }
        self.streamed_text_in_block = false;
        self.text_chunks.push(text);
    }

    fn on_thinking_done(&mut self, thinking: String, signature: Option<String>) {
        // Same dedup rationale as the Message arm. Always
        // emit the `ThinkingComplete` marker so consumers
        // that pattern-match on the end-of-thinking event
        // (e.g. the chat client's "Thought for Xs" pill)
        // still see the close signal.
        if !self.streamed_thinking_in_block {
            self.thinking_bytes += thinking.len();
            emit_event(
                self.ctx.event_tx,
                AgentLoopEvent::ThinkingDelta(thinking.clone()),
            );
        }
        self.streamed_thinking_in_block = false;
        emit_event(self.ctx.event_tx, AgentLoopEvent::ThinkingComplete);
        self.thinking_chunks.push((thinking, signature));
    }

    fn on_tool_use_start(&mut self, id: String, name: String) {
        self.liveness
            .beat(StreamPhase::ToolInput, self.ctx.event_tx);
        emit_event(
            self.ctx.event_tx,
            AgentLoopEvent::ToolStart {
                id: id.clone(),
                name: name.clone(),
            },
        );
        self.open_tool = Some(OpenToolCall {
            id,
            name,
            input_json: String::new(),
            last_snapshot: tokio::time::Instant::now(),
        });
    }

    fn on_tool_input_delta(&mut self, chunk: &str) {
        self.liveness
            .beat(StreamPhase::ToolInput, self.ctx.event_tx);
        if let Some(tool) = self.open_tool.as_mut() {
            tool.input_json.push_str(chunk);
            if tool.last_snapshot.elapsed() >= TOOL_INPUT_SNAPSHOT_INTERVAL {
                tool.last_snapshot = tokio::time::Instant::now();
                emit_event(
                    self.ctx.event_tx,
                    AgentLoopEvent::ToolInputSnapshot {
                        id: tool.id.clone(),
                        name: tool.name.clone(),
                        input: tool.input_json.clone(),
                    },
                );
            }
        }
    }
}
