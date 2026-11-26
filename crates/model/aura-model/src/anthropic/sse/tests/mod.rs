//! Unit tests for the SSE submodules.
//!
//! Covers:
//!
//! - [`super::parse::parse_sse_event`] edge cases (ping, malformed
//!   JSON, missing data line, Anthropic-shape `error.type`
//!   passthrough, content-block deltas, message-delta stop reasons).
//! - [`super::state::SseStream`] state-machine behaviour: synthetic
//!   `HttpMeta` preamble, partial-chunk reassembly, CRLF tolerance,
//!   error-as-terminal, multi-event blocks.

use super::parse::parse_sse_event;
use super::state::SseStream;
use crate::{StopReason, StreamAccumulator, StreamEvent, Usage};
use futures_util::{Stream, StreamExt};

fn bytes_stream(
    chunks: Vec<&'static str>,
) -> impl Stream<Item = Result<bytes::Bytes, std::io::Error>> + Unpin {
    futures_util::stream::iter(
        chunks
            .into_iter()
            .map(|c| Ok(bytes::Bytes::from(c.to_string()))),
    )
}

// --- message_delta usage totals (OpenRouter non-Anthropic upstreams) ---

/// Parse each SSE block with the production parser, feed the events
/// through a [`StreamAccumulator`] and return the final usage.
fn usage_from_sse_blocks(blocks: &[&str]) -> Usage {
    let mut acc = StreamAccumulator::new();
    for block in blocks {
        let event = parse_sse_event(block).expect("SSE block yields an event");
        assert!(
            !matches!(event, StreamEvent::Error { .. }),
            "block failed to parse: {block} -> {event:?}"
        );
        acc.process(&event);
    }
    acc.into_response(0, 0)
        .expect("stream finishes cleanly")
        .usage
}

const OPENROUTER_START_ZERO: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"gen-1","type":"message","role":"assistant","content":[],"model":"openai/gpt-5","usage":{"input_tokens":0,"output_tokens":0}}}"#;

const ANTHROPIC_START: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-sonnet-4","usage":{"input_tokens":1234,"output_tokens":1,"cache_creation_input_tokens":100,"cache_read_input_tokens":900}}}"#;

const MESSAGE_STOP: &str = "event: message_stop\ndata: {\"type\":\"message_stop\"}";

// --- SseStream tests ---

/// Helper: consume and assert the synthetic `HttpMeta` frame that
/// every `SseStream` now emits before any provider event. Keeps
/// the rest of the SSE body tests focused on protocol parsing
/// instead of the transport preamble.
async fn expect_http_meta<S>(stream: &mut SseStream<S>, expected_request_id: Option<&str>)
where
    S: Stream<Item = Result<bytes::Bytes, std::io::Error>> + Unpin,
{
    match stream.next().await {
        Some(Ok(StreamEvent::HttpMeta { request_id, .. })) => {
            assert_eq!(request_id.as_deref(), expected_request_id);
        }
        other => panic!("Expected HttpMeta preamble, got {other:?}"),
    }
}

mod parse;
mod sse_stream;
mod usage_totals;
