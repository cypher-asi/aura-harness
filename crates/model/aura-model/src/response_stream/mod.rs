//! Provider-neutral high-level streaming event surface (Layer E.3).
//!
//! Codex's `ResponseEvent` enum (see [`turn.rs:1747`](
//! https://github.com/.../codex-rs/core/src/session/turn.rs)) is the
//! shape the codex sampling driver consumes from any model provider:
//! it surfaces *completed* top-level items (assistant message, tool
//! use, thinking block) as they become available, rather than raw
//! per-delta wire frames. The agent loop's stream pump consumes this
//! surface so it can spawn a tool future the moment a `tool_use`
//! item finishes, instead of waiting for the whole response to
//! finalize.
//!
//! # Module-level invariants (Rule 13)
//!
//! - **Ordering**: `OutputItemDone` events arrive in the same order
//!   the content blocks appear in the model's final response. Tools
//!   pushed into a downstream `FuturesOrdered` therefore drain in
//!   submission order (the codex `drain_in_flight` FIFO contract).
//! - **Exactly one `Completed`**: every stream that reaches the end
//!   of its model response emits exactly one [`ResponseEvent::Completed`]
//!   before terminating with `None`. A transport-level abort skips the
//!   `Completed` event and surfaces as `Some(Err(StreamError::…))`.
//! - **No `anyhow`**: this is a library crate per Rule 4.2; the
//!   [`StreamError`] enum derives [`thiserror::Error`] and every
//!   variant carries enough context to be actionable in logs.
//! - **No `unwrap()` outside `#[cfg(test)]`**: the adapter constructs
//!   events from already-validated wire frames; any decode failures
//!   are surfaced via [`StreamError::InvalidEvent`] rather than
//!   panicking.

mod adapter;

use std::pin::Pin;

use futures_util::Stream;
use serde_json::Value;

use crate::types::{ContentBlock, PartialToolUse, StopReason, Usage};
use crate::ModelResponse;

pub use adapter::response_stream_from_event_stream;

/// A complete top-level item emitted by the model during streaming.
///
/// Maps 1:1 onto Anthropic's `content_block_*` event family after the
/// adapter has aggregated per-delta frames into a finished block.
#[derive(Debug, Clone)]
pub enum OutputItem {
    /// A finished assistant text block.
    Message {
        /// The accumulated text content of the block.
        text: String,
    },
    /// A finished thinking block (extended thinking).
    Thinking {
        /// The accumulated thinking text.
        thinking: String,
        /// Optional signature for echoing back to the API.
        signature: Option<String>,
    },
    /// A finished tool-use block. The downstream sampling driver
    /// spawns the tool future the moment this variant arrives.
    ToolUse {
        /// Provider-side tool-use id (e.g. `toolu_01…`).
        id: String,
        /// Tool name (e.g. `read_file`).
        name: String,
        /// Decoded tool-input JSON. Parsed once by the adapter so
        /// every downstream consumer sees the same parse result.
        input: Value,
    },
    /// A closed tool block whose arguments could not be decoded. Never execute it.
    InvalidToolUse {
        id: String,
        name: String,
        reason: String,
    },
}

/// Recoverable provider/transport failure surfaced inside a
/// [`ResponseEventStream`]. Library crate => `thiserror` (Rule 4.2).
///
/// `Timeout` is emitted by the *agent-side* pump, not by the adapter
/// itself — the adapter has no notion of deadlines. It lives here so
/// the agent loop can route both transport and timeout failures
/// through a single error type when it surfaces them through
/// `AgentError::Stream` (Rule 4.3).
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// The underlying transport (HTTP / SSE / mock channel) closed
    /// before the model emitted a [`ResponseEvent::Completed`] frame.
    #[error("transport closed before stream completed: {context}")]
    TransportClosed {
        /// Human-readable context describing where the close happened
        /// (e.g. `"after content_block_start for tool_use"`).
        context: String,
    },
    /// The transport delivered a syntactically-valid event whose
    /// shape was unexpected (e.g. a `tool_use` block whose
    /// `input_json` failed to decode).
    #[error("provider returned invalid event shape: {reason}")]
    InvalidEvent {
        /// Human-readable reason for the rejection.
        reason: String,
    },
    /// The agent-side pump's `stream_event_timeout` elapsed waiting
    /// for the next [`ResponseEvent`]. Carries `elapsed_ms` so the
    /// surfacing surface (CLI, dashboards) can correlate the failure
    /// with the configured boundary policy.
    #[error("stream timed out after {elapsed_ms}ms waiting for next event")]
    Timeout {
        /// Configured boundary timeout in milliseconds.
        elapsed_ms: u64,
    },
    /// The stream aborted while a `tool_use` block was still being
    /// accumulated. Carries the in-flight tool identity so agent
    /// callers can retry the streaming request without losing which
    /// write/edit attempt was interrupted.
    #[error("{reason}")]
    StreamAbortedWithPartial {
        /// Human-readable provider/transport failure context.
        reason: String,
        /// In-flight tool-use captured just before the stream died.
        partial_tool_use: Option<PartialToolUse>,
    },
}

/// Coarse classification of the raw streaming frame that produced a
/// [`ResponseEvent::Keepalive`]. Lets a consumer log *what* the model
/// is doing during a long turn (e.g. a multi-second extended-thinking
/// block) without re-deriving it from the swallowed wire frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamPhase {
    /// Stream is open but no content block has started yet
    /// (`message_start` / HTTP metadata / inter-block `message_delta`).
    Connecting,
    /// An extended-thinking block is streaming (`thinking` /
    /// `signature` deltas, or a thinking `content_block_start`).
    Thinking,
    /// An assistant text block is streaming (`text` deltas or a text
    /// `content_block_start`).
    Text,
    /// A `tool_use` block's input JSON is streaming (`input_json`
    /// deltas or a tool_use `content_block_start`).
    ToolInput,
    /// A provider keepalive `ping` — pure liveness, no content phase.
    Ping,
}

/// Provider-neutral high-level event emitted while the model is
/// streaming. Mirrors codex's `ResponseEvent` enum.
#[derive(Debug)]
pub enum ResponseEvent {
    /// A complete top-level item arrived. The downstream sampling
    /// driver dispatches based on the inner [`OutputItem`] variant
    /// (e.g. spawn a tool future on `ToolUse`).
    OutputItemDone(OutputItem),
    /// A raw transport/provider frame (ping or intra-block delta)
    /// arrived that did not complete a top-level item. Carries no
    /// content — only a liveness signal plus the current
    /// [`StreamPhase`] — so a consumer can reset a per-event timeout
    /// (a healthy but slow stream is not a dead one) and surface the
    /// streaming phase to operators. The agent-side pump treats this
    /// as a no-op for accumulation/ordering purposes.
    Keepalive(StreamPhase),
    /// An intra-block assistant-text delta. Carries the incremental
    /// text chunk so the agent-side pump can stream it to the client
    /// in real time instead of waiting for the whole block to close
    /// (the previous behaviour, which surfaced these as content-free
    /// `Keepalive(StreamPhase::Text)` and dropped the text). Also
    /// doubles as a liveness signal: receiving it resets the pump's
    /// per-event timeout. The accumulator still builds the full block
    /// for final-response synthesis; this variant is purely for
    /// incremental surfacing.
    OutputTextDelta(String),
    /// An intra-block extended-thinking delta. Same incremental-
    /// surfacing + liveness rationale as [`Self::OutputTextDelta`],
    /// for the thinking channel. The terminal
    /// [`OutputItem::Thinking`] still arrives at `content_block_stop`
    /// so consumers that key off block close (e.g. the "Thought for
    /// Xs" pill) keep working.
    OutputThinkingDelta(String),
    /// A `tool_use` block opened. Lets the client show the call while
    /// its arguments stream in; the executable call still arrives as
    /// [`OutputItem::ToolUse`] at `content_block_stop`.
    OutputToolUseStart {
        /// Provider-side tool-use id (e.g. `toolu_01…`).
        id: String,
        /// Tool name (e.g. `write_file`).
        name: String,
    },
    /// A fragment of the open `tool_use` block's input JSON. Appending
    /// the fragments in order gives the arguments so far.
    OutputToolInputDelta(String),
    /// Terminal event for this sampling request. `end_turn` is `Some(true)`
    /// when the model signalled it intends to stop, `Some(false)` when
    /// it explicitly signalled more work, and `None` when the provider
    /// did not include the field (legacy / mock paths).
    Completed {
        /// `Some(false)` indicates the model is *not* ending its turn
        /// (further sampling needed). `None` is the default for
        /// providers that don't surface this bit.
        end_turn: Option<bool>,
        /// Exact provider termination signal; unlike `end_turn`, preserves truncation.
        stop_reason: Option<StopReason>,
        /// Per-response usage counters as reported by the provider.
        usage: Usage,
    },
    /// Recoverable provider error surfaced as an in-band event so
    /// the pump can still decide whether to retry or abort. Transport-
    /// level failures arrive as `Stream::Item = Err(StreamError::…)`
    /// instead; both paths flow through `AgentError::Stream`.
    Error(StreamError),
}

/// Boxed `Stream` of [`ResponseEvent`] items used by `ModelProvider`
/// implementations. Mirrors the [`StreamEventStream`](crate::StreamEventStream) alias for the
/// lower-level per-delta surface.
pub type ResponseEventStream =
    Pin<Box<dyn Stream<Item = Result<ResponseEvent, StreamError>> + Send + 'static>>;

/// Build a `ResponseEventStream` that synthesises events from a
/// fully-buffered [`ModelResponse`].
///
/// The synthesised stream emits one [`ResponseEvent::OutputItemDone`]
/// per content block (text / thinking / tool_use) followed by a
/// terminal [`ResponseEvent::Completed`]. Used as the fallback path
/// inside the default `complete_response_stream` trait impl for
/// providers that have not been wired to emit incremental events yet.
#[must_use]
pub fn response_stream_from_response(response: ModelResponse) -> ResponseEventStream {
    let mut events: Vec<Result<ResponseEvent, StreamError>> = Vec::new();
    for block in response.message.content {
        match block {
            ContentBlock::Text { text } => {
                events.push(Ok(ResponseEvent::OutputItemDone(OutputItem::Message {
                    text,
                })));
            }
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                events.push(Ok(ResponseEvent::OutputItemDone(OutputItem::Thinking {
                    thinking,
                    signature,
                })));
            }
            ContentBlock::ToolUse { id, name, input } => {
                events.push(Ok(ResponseEvent::OutputItemDone(OutputItem::ToolUse {
                    id,
                    name,
                    input,
                })));
            }
            ContentBlock::Image { .. } | ContentBlock::ToolResult { .. } => {}
        }
    }
    let end_turn = match response.stop_reason {
        StopReason::EndTurn | StopReason::StopSequence => Some(true),
        StopReason::ToolUse | StopReason::MaxTokens => Some(false),
    };
    events.push(Ok(ResponseEvent::Completed {
        end_turn,
        stop_reason: Some(response.stop_reason),
        usage: response.usage,
    }));
    Box::pin(futures_util::stream::iter(events))
}

#[cfg(test)]
mod tests;
