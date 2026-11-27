//! Forwarding agent-loop events onto the WebSocket wire protocol.

use crate::protocol::{
    ErrorMsg, IterationComplete, OutboundMessage, TextDelta, ThinkingDelta, ToolCallSnapshot,
    ToolResultMsg, ToolUseStart,
};
use async_trait::async_trait;
use aura_agent::{map_agent_loop_event, AgentLoopEvent, DebugEvent, TurnEventSink};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{timeout, timeout_at, Duration, Instant};
use tracing::warn;

const OUTBOUND_DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);
const STREAM_DELTA_COALESCE_BYTES: usize = 512;
/// How long a delta may wait for more of its kind. A model that writes
/// slowly must still stream, not arrive [`STREAM_DELTA_COALESCE_BYTES`] at
/// a time or whole once its block ends.
const STREAM_DELTA_FLUSH: Duration = Duration::from_millis(33);

/// [`TurnEventSink`] that maps events onto the WebSocket wire protocol.
///
/// Phase 3 consolidated the handwritten match here and its sibling in
/// the TUI's `UiCommandSink` into a single dispatcher
/// ([`map_agent_loop_event`]). Each sink overrides only the hooks it
/// cares about; unhandled variants fall through to no-op defaults,
/// but the dispatcher's match is exhaustive, so adding a new
/// [`AgentLoopEvent`] variant is still a compile error until every
/// consumer has handled it.
///
/// The WS sink awaits bounded mpsc capacity so transient saturation
/// applies backpressure instead of dropping later terminal frames.
struct OutboundMessageSink<'a> {
    outbound: &'a mpsc::Sender<OutboundMessage>,
    closed: bool,
    pending_delta: Option<PendingStreamDelta>,
    /// When `pending_delta` goes out, however short it is.
    flush_at: Option<Instant>,
}

enum PendingStreamDelta {
    Text(String),
    Thinking(String),
}

impl PendingStreamDelta {
    fn len(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            Self::Thinking(thinking) => thinking.len(),
        }
    }

    fn into_message(self) -> OutboundMessage {
        match self {
            Self::Text(text) => OutboundMessage::TextDelta(TextDelta { text }),
            Self::Thinking(thinking) => OutboundMessage::ThinkingDelta(ThinkingDelta { thinking }),
        }
    }
}

impl OutboundMessageSink<'_> {
    async fn push(&mut self, msg: OutboundMessage) {
        self.flush_pending_delta().await;
        self.push_now(msg).await;
    }

    async fn push_text_delta(&mut self, text: String) {
        self.push_delta(StreamDeltaKind::Text, text).await;
    }

    async fn push_thinking_delta(&mut self, thinking: String) {
        self.push_delta(StreamDeltaKind::Thinking, thinking).await;
    }

    async fn push_delta(&mut self, kind: StreamDeltaKind, chunk: String) {
        if self.closed {
            return;
        }

        let matches_pending = matches!(
            (&self.pending_delta, kind),
            (Some(PendingStreamDelta::Text(_)), StreamDeltaKind::Text)
                | (
                    Some(PendingStreamDelta::Thinking(_)),
                    StreamDeltaKind::Thinking
                )
        );
        if self.pending_delta.is_some() && !matches_pending {
            self.flush_pending_delta().await;
        }
        if self.pending_delta.is_none() {
            self.flush_at = Some(Instant::now() + STREAM_DELTA_FLUSH);
        }

        match (&mut self.pending_delta, kind) {
            (Some(PendingStreamDelta::Text(buffer)), StreamDeltaKind::Text) => {
                buffer.push_str(&chunk);
            }
            (Some(PendingStreamDelta::Thinking(buffer)), StreamDeltaKind::Thinking) => {
                buffer.push_str(&chunk);
            }
            (slot @ None, StreamDeltaKind::Text) => {
                *slot = Some(PendingStreamDelta::Text(chunk));
            }
            (slot @ None, StreamDeltaKind::Thinking) => {
                *slot = Some(PendingStreamDelta::Thinking(chunk));
            }
            _ => unreachable!("mismatched pending delta was flushed before append"),
        }

        if self
            .pending_delta
            .as_ref()
            .is_some_and(|delta| delta.len() >= STREAM_DELTA_COALESCE_BYTES)
        {
            self.flush_pending_delta().await;
        }
    }

    async fn flush_pending_delta(&mut self) {
        self.flush_at = None;
        if self.closed {
            return;
        }
        if let Some(delta) = self.pending_delta.take() {
            self.push_now(delta.into_message()).await;
        }
    }

    /// The next agent-loop event, sending the pending delta while it
    /// waits once the delta's time comes; `None` once the events end or
    /// the socket is gone.
    async fn next_event(
        &mut self,
        events: &mut mpsc::Receiver<AgentLoopEvent>,
    ) -> Option<AgentLoopEvent> {
        while let Some(flush_at) = self.flush_at {
            match timeout_at(flush_at, events.recv()).await {
                Ok(event) => return event,
                Err(_) => self.flush_pending_delta().await,
            }
        }
        if self.closed {
            return None;
        }
        events.recv().await
    }

    async fn push_now(&mut self, msg: OutboundMessage) {
        if self.closed {
            return;
        }
        if !send_outbound_with_backpressure(self.outbound, msg, "turn_event").await {
            self.closed = true;
        }
    }
}

#[derive(Clone, Copy)]
enum StreamDeltaKind {
    Text,
    Thinking,
}

pub(in crate::gateway::session) async fn send_outbound_with_backpressure(
    outbound: &mpsc::Sender<OutboundMessage>,
    msg: OutboundMessage,
    context: &'static str,
) -> bool {
    match timeout(OUTBOUND_DELIVERY_TIMEOUT, outbound.send(msg)).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) => {
            warn!(context, "Outbound channel closed while delivering message");
            false
        }
        Err(_) => {
            warn!(
                context,
                timeout_ms =
                    u64::try_from(OUTBOUND_DELIVERY_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
                "Timed out delivering outbound message"
            );
            false
        }
    }
}

#[async_trait]
impl TurnEventSink for OutboundMessageSink<'_> {
    async fn on_text_delta(&mut self, text: String) {
        self.push_text_delta(text).await;
    }

    async fn on_thinking_delta(&mut self, thinking: String) {
        self.push_thinking_delta(thinking).await;
    }

    async fn on_stream_reset(&mut self, reason: String, text_bytes: usize, thinking_bytes: usize) {
        self.push(OutboundMessage::Progress(crate::protocol::ProgressMsg {
            stage: "stream_reset".to_string(),
            tool_name: None,
            elapsed_ms: None,
            message: Some(reason),
            reset_text_bytes: Some(text_bytes as u64),
            reset_thinking_bytes: Some(thinking_bytes as u64),
        }))
        .await;
    }

    async fn on_tool_start(&mut self, id: String, name: String) {
        self.push(OutboundMessage::ToolUseStart(ToolUseStart { id, name }))
            .await;
    }

    async fn on_tool_result(
        &mut self,
        tool_use_id: String,
        tool_name: String,
        content: String,
        is_error: bool,
        image: Option<aura_core_types::ToolResultImage>,
    ) {
        // Split the typed image into the flat wire fields aura-os
        // persists. Never log the base64 payload — only dims/length are
        // safe (the executor already logged dims on capture).
        let (image_base64, image_media_type) = match image {
            Some(img) => (Some(img.base64), Some(img.media_type)),
            None => (None, None),
        };
        self.push(OutboundMessage::ToolResult(ToolResultMsg {
            name: tool_name,
            result: content,
            is_error,
            tool_use_id: Some(tool_use_id),
            image_base64,
            image_media_type,
        }))
        .await;
    }

    async fn on_tool_input_snapshot(&mut self, id: String, name: String, input: String) {
        // While streaming with `eager_input_streaming`, `input` is
        // partial JSON like `{"title":"Hi","markdown_contents":"# H`.
        // A strict `serde_json::from_str` would fail and yield `{}`,
        // making every mid-stream snapshot useless to the UI. Use a
        // tool-aware partial-JSON extractor that pulls out the
        // best-effort value of well-known string fields the preview
        // cards consume (markdown_contents, content, old_text, etc.).
        let parsed = crate::gateway::session::partial_json::parse_partial_tool_input(&name, &input);
        let str_field_len =
            |key: &str| parsed.get(key).and_then(|v| v.as_str()).map_or(0, str::len);
        let md_len = str_field_len("markdown_contents");
        let content_len = str_field_len("content");
        let description_len = str_field_len("description");
        tracing::info!(
            tool = %name,
            raw_input_bytes = input.len(),
            parsed_keys = parsed.as_object().map_or(0, |o| o.len()),
            markdown_len = md_len,
            content_len,
            description_len,
            "forwarding tool_call_snapshot"
        );
        self.push(OutboundMessage::ToolCallSnapshot(ToolCallSnapshot {
            id,
            name,
            input: parsed,
        }))
        .await;
    }

    async fn on_error(&mut self, code: String, message: String, recoverable: bool) {
        self.push(OutboundMessage::Error(ErrorMsg {
            code,
            message,
            recoverable,
            support_id: None,
        }))
        .await;
    }

    async fn on_progress(
        &mut self,
        stage: String,
        tool_name: Option<String>,
        elapsed_ms: Option<u64>,
        message: Option<String>,
    ) {
        self.push(OutboundMessage::Progress(crate::protocol::ProgressMsg {
            stage,
            tool_name,
            elapsed_ms,
            message,
            reset_text_bytes: None,
            reset_thinking_bytes: None,
        }))
        .await;
    }

    async fn on_iteration_complete(
        &mut self,
        iteration: usize,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        self.push(OutboundMessage::IterationComplete(IterationComplete {
            iteration: iteration as u64,
            input_tokens,
            output_tokens,
        }))
        .await;
    }

    // The following variants are intentional no-ops on the WS wire —
    // `ToolComplete`, `ThinkingComplete`, `StepComplete`, `Warning`,
    // `Debug`. The trait defaults cover them, but the mapper's
    // exhaustive match still forces a decision here whenever the event
    // enum changes.
    async fn on_debug(&mut self, _event: DebugEvent) {}
}

pub(in crate::gateway::session) async fn forward_events_to_ws(
    event_rx: mpsc::Receiver<AgentLoopEvent>,
    outbound: mpsc::Sender<OutboundMessage>,
) {
    forward_and_record(event_rx, outbound, None).await;
}

/// [`forward_events_to_ws`], also recording each event to `recorder`.
pub(in crate::gateway::session) async fn forward_and_record(
    mut event_rx: mpsc::Receiver<AgentLoopEvent>,
    outbound: mpsc::Sender<OutboundMessage>,
    recorder: Option<Arc<aura_agent_trace::RunRecorder>>,
) {
    let mut sink = OutboundMessageSink {
        outbound: &outbound,
        closed: false,
        pending_delta: None,
        flush_at: None,
    };
    while let Some(event) = sink.next_event(&mut event_rx).await {
        if let Some(rec) = &recorder {
            rec.on_event(&event);
        }
        map_agent_loop_event(event, &mut sink).await;
        if sink.closed {
            break;
        }
    }
    sink.flush_pending_delta().await;
}
