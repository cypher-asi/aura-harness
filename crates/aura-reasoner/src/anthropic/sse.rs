use super::api_types::{SseContentBlock, SseDelta, SseEvent};
use crate::error::ReasonerError;
use crate::{StopReason, StreamContentType, StreamEvent};
use futures_util::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

const MAX_SSE_BUFFER_SIZE: usize = 10 * 1024 * 1024;

/// A stream that parses SSE events from an HTTP byte stream.
///
/// On first poll, the stream yields a synthetic
/// [`StreamEvent::HttpMeta`] carrying the HTTP `x-request-id` (if the
/// caller captured one before consuming the response body). Subsequent
/// polls emit the provider's SSE events as usual. This is the only
/// path that surfaces the request id from the *streaming* HTTP
/// response — the provider's wire protocol never includes it inside
/// the SSE body, and the response headers are gone once
/// `response.bytes_stream()` has been called, so the capture has to
/// happen at the HTTP layer and then flow through this type.
pub(super) struct SseStream<S> {
    inner: S,
    buffer: String,
    finished: bool,
    /// `x-request-id` from the HTTP response headers, set by
    /// [`SseStream::with_request_id`]. `None` means the caller didn't
    /// capture one (or the upstream didn't send one).
    request_id: Option<String>,
    /// Whether the synthetic `HttpMeta` event has already been
    /// emitted. Ensures we only surface it once, before any provider
    /// event.
    emitted_http_meta: bool,
}

impl<S> SseStream<S> {
    /// Retained for test-only construction of a stream without header
    /// capture. Production call sites go through
    /// [`Self::with_request_id`] so the synthetic `HttpMeta` preamble
    /// actually carries the HTTP `x-request-id`.
    #[cfg(test)]
    pub(super) const fn new(inner: S) -> Self {
        Self {
            inner,
            buffer: String::new(),
            finished: false,
            request_id: None,
            emitted_http_meta: false,
        }
    }

    /// Construct a stream that will emit a synthetic
    /// [`StreamEvent::HttpMeta`] carrying `request_id` before the
    /// first provider event.
    pub(super) const fn with_request_id(inner: S, request_id: Option<String>) -> Self {
        Self {
            inner,
            buffer: String::new(),
            finished: false,
            request_id,
            emitted_http_meta: false,
        }
    }
}

impl<S, E> Stream for SseStream<S>
where
    S: Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    E: std::error::Error + Send + Sync + 'static,
{
    type Item = Result<StreamEvent, ReasonerError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.finished {
            return Poll::Ready(None);
        }

        // Emit the synthetic HttpMeta event once, before any provider
        // event. Doing this before `parse_next_event` guarantees a
        // consumer that inspects the first event (e.g. to seed a
        // `StreamAccumulator`) never has to race against the first
        // `message_start`.
        if !self.emitted_http_meta {
            self.emitted_http_meta = true;
            let request_id = self.request_id.clone();
            return Poll::Ready(Some(Ok(StreamEvent::HttpMeta { request_id })));
        }

        loop {
            if let Some(event) = self.parse_next_event() {
                if matches!(event, StreamEvent::MessageStop | StreamEvent::Error { .. }) {
                    self.finished = true;
                }
                return Poll::Ready(Some(Ok(event)));
            }

            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    match std::str::from_utf8(&bytes) {
                        Ok(s) => self.buffer.push_str(s),
                        Err(e) => {
                            return Poll::Ready(Some(Ok(StreamEvent::Error {
                                message: format!("invalid UTF-8 in SSE stream: {e}"),
                                request_id: None,
                            })));
                        }
                    }
                    if self.buffer.len() > MAX_SSE_BUFFER_SIZE {
                        self.finished = true;
                        return Poll::Ready(Some(Err(ReasonerError::Internal(format!(
                            "SSE buffer exceeded {MAX_SSE_BUFFER_SIZE} bytes"
                        )))));
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    self.finished = true;
                    return Poll::Ready(Some(Err(ReasonerError::Request(format!(
                        "Stream error: {e}"
                    )))));
                }
                Poll::Ready(None) => {
                    self.finished = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<S> SseStream<S> {
    /// Try to parse the next complete SSE event from the buffer.
    fn parse_next_event(&mut self) -> Option<StreamEvent> {
        let event_end = self
            .buffer
            .find("\n\n")
            .or_else(|| self.buffer.find("\r\n\r\n"));

        let event_end = event_end?;
        let event_str = self.buffer[..event_end].to_string();

        let delimiter_len = if self.buffer[event_end..].starts_with("\r\n\r\n") {
            4
        } else {
            2
        };
        self.buffer = self.buffer[event_end + delimiter_len..].to_string();

        parse_sse_event(&event_str)
    }
}

fn parse_sse_event(event_str: &str) -> Option<StreamEvent> {
    let mut event_type = None;
    let mut data = None;

    for line in event_str.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(suffix) = line.strip_prefix("event:") {
            event_type = Some(suffix.trim().to_string());
        } else if let Some(suffix) = line.strip_prefix("data:") {
            data = Some(suffix.trim().to_string());
        }
    }

    let data = data?;

    if event_type.as_deref() == Some("ping") {
        return Some(StreamEvent::Ping);
    }

    let sse_event: SseEvent = match serde_json::from_str(&data) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "malformed SSE JSON payload");
            return Some(StreamEvent::Error {
                message: format!("malformed SSE JSON: {e}"),
                request_id: None,
            });
        }
    };

    match sse_event {
        SseEvent::MessageStart { message } => Some(StreamEvent::MessageStart {
            message_id: message.id,
            model: message.model,
            input_tokens: message.usage.as_ref().map(|u| u.input_tokens),
            cache_creation_input_tokens: message
                .usage
                .as_ref()
                .and_then(|u| u.cache_creation_input_tokens),
            cache_read_input_tokens: message
                .usage
                .as_ref()
                .and_then(|u| u.cache_read_input_tokens),
        }),
        SseEvent::ContentBlockStart {
            index,
            content_block,
        } => {
            let content_type = match content_block {
                SseContentBlock::Text { .. } => StreamContentType::Text,
                SseContentBlock::Thinking { .. } => StreamContentType::Thinking,
                SseContentBlock::ToolUse { id, name } => StreamContentType::ToolUse { id, name },
            };
            Some(StreamEvent::ContentBlockStart {
                index,
                content_type,
            })
        }
        SseEvent::ContentBlockDelta { delta, .. } => match delta {
            SseDelta::Text { text } => Some(StreamEvent::TextDelta { text }),
            SseDelta::Thinking { thinking } => Some(StreamEvent::ThinkingDelta { thinking }),
            SseDelta::Signature { signature } => Some(StreamEvent::SignatureDelta { signature }),
            SseDelta::InputJson { partial_json } => {
                Some(StreamEvent::InputJsonDelta { partial_json })
            }
        },
        SseEvent::ContentBlockStop { index } => Some(StreamEvent::ContentBlockStop { index }),
        SseEvent::MessageDelta { delta, usage } => {
            let stop_reason = delta.stop_reason.as_deref().map(|s| match s {
                "tool_use" => StopReason::ToolUse,
                "max_tokens" => StopReason::MaxTokens,
                "stop_sequence" => StopReason::StopSequence,
                _ => StopReason::EndTurn,
            });
            Some(StreamEvent::MessageDelta {
                stop_reason,
                output_tokens: usage.map_or(0, |u| u.output_tokens),
            })
        }
        SseEvent::MessageStop => Some(StreamEvent::MessageStop),
        SseEvent::Ping => Some(StreamEvent::Ping),
        SseEvent::Error { error } => {
            // Preserve the Anthropic-shape `error.type` in the message so
            // downstream classification (retry policy, UI labeling) can
            // distinguish `overloaded_error` from `api_error` / generic
            // `Internal server error`. Proxies often inject a bare
            // `Internal server error` with no `type`, in which case we
            // fall back to the raw message.
            let request_id = error.request_id.clone();
            let message = match error.error_type.as_deref() {
                Some(kind) if !kind.is_empty() => format!("{kind}: {}", error.message),
                _ => error.message,
            };
            Some(StreamEvent::Error {
                message,
                request_id,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    fn bytes_stream(
        chunks: Vec<&'static str>,
    ) -> impl Stream<Item = Result<bytes::Bytes, std::io::Error>> + Unpin {
        futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|c| Ok(bytes::Bytes::from(c.to_string()))),
        )
    }

    // --- parse_sse_event unit tests ---

    #[test]
    fn test_parse_ping_event() {
        let event = parse_sse_event("event: ping\ndata: {}");
        assert!(matches!(event, Some(StreamEvent::Ping)));
    }

    #[test]
    fn test_parse_event_without_data_returns_none() {
        let event = parse_sse_event("event: message_start");
        assert!(event.is_none());
    }

    #[test]
    fn test_parse_event_with_invalid_json_returns_error() {
        let event = parse_sse_event("event: message_start\ndata: {not valid json!!}");
        assert!(
            matches!(event, Some(StreamEvent::Error { ref message, .. }) if message.contains("malformed SSE JSON")),
            "expected StreamEvent::Error, got {event:?}"
        );
    }

    #[test]
    fn test_parse_message_stop_event() {
        let event = parse_sse_event("event: message_stop\ndata: {\"type\":\"message_stop\"}");
        assert!(matches!(event, Some(StreamEvent::MessageStop)));
    }

    #[test]
    fn test_parse_error_event() {
        let event = parse_sse_event(
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"message\":\"overloaded\"}}",
        );
        match event {
            Some(StreamEvent::Error {
                message,
                request_id,
            }) => {
                assert_eq!(message, "overloaded");
                assert_eq!(request_id, None);
            }
            other => panic!("Expected Error event, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_sseerror_with_request_id_field_in_body() {
        // Some proxies (notably `aura-router`) embed the originating
        // request id inside the SSE error body. Forward it on
        // `StreamEvent::Error.request_id` so the accumulator can adopt
        // it when the response-header `x-request-id` was unavailable.
        let event = parse_sse_event(
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"Internal server error\",\"request_id\":\"req_01XYZ\"}}",
        );
        match event {
            Some(StreamEvent::Error {
                message,
                request_id,
            }) => {
                assert_eq!(message, "api_error: Internal server error");
                assert_eq!(request_id.as_deref(), Some("req_01XYZ"));
            }
            other => panic!("Expected Error event, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_error_event_preserves_anthropic_error_type() {
        // Anthropic SSE wire format: `error.type` distinguishes
        // `overloaded_error` (retryable per-provider) from
        // `api_error` (generic 5xx). The reasoner's downstream retry
        // policy keys off this string, so the parser must forward it.
        let event = parse_sse_event(
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"service is overloaded\"}}",
        );
        match event {
            Some(StreamEvent::Error { message, .. }) => {
                assert_eq!(message, "overloaded_error: service is overloaded");
            }
            other => panic!("Expected Error event, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_error_event_without_type_uses_raw_message() {
        // Proxies sometimes emit a bare `{"error":{"message":"..."}}`
        // with no `type` — preserve the raw message verbatim so the
        // downstream classifier still sees the original prose.
        let event = parse_sse_event(
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"message\":\"Internal server error\"}}",
        );
        match event {
            Some(StreamEvent::Error { message, .. }) => {
                assert_eq!(message, "Internal server error");
            }
            other => panic!("Expected Error event, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_content_block_delta_text() {
        let event = parse_sse_event(
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}",
        );
        match event {
            Some(StreamEvent::TextDelta { text }) => assert_eq!(text, "hi"),
            other => panic!("Expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_message_delta_stop_reasons() {
        for (reason_str, expected) in [
            ("tool_use", StopReason::ToolUse),
            ("max_tokens", StopReason::MaxTokens),
            ("stop_sequence", StopReason::StopSequence),
            ("end_turn", StopReason::EndTurn),
            ("unknown_reason", StopReason::EndTurn),
        ] {
            let data = format!(
                "event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"{reason_str}\"}},\"usage\":{{\"output_tokens\":42}}}}"
            );
            let event = parse_sse_event(&data);
            match event {
                Some(StreamEvent::MessageDelta {
                    stop_reason,
                    output_tokens,
                }) => {
                    assert_eq!(stop_reason, Some(expected));
                    assert_eq!(output_tokens, 42);
                }
                other => panic!("Expected MessageDelta, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_parse_message_delta_no_usage() {
        let event = parse_sse_event(
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":null},\"usage\":null}",
        );
        match event {
            Some(StreamEvent::MessageDelta { output_tokens, .. }) => {
                assert_eq!(output_tokens, 0);
            }
            other => panic!("Expected MessageDelta, got {other:?}"),
        }
    }

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
            Some(Ok(StreamEvent::HttpMeta { request_id })) => {
                assert_eq!(request_id.as_deref(), expected_request_id);
            }
            other => panic!("Expected HttpMeta preamble, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sse_stream_emits_http_meta_first() {
        let inner = bytes_stream(vec![
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ]);
        let mut stream = SseStream::with_request_id(inner, Some("req_01ABC".to_string()));
        // First event must be the synthetic HttpMeta, before any
        // provider event — otherwise `StreamAccumulator` can't seed
        // `provider_request_id` when `message_start` arrives.
        match stream.next().await {
            Some(Ok(StreamEvent::HttpMeta { request_id })) => {
                assert_eq!(request_id.as_deref(), Some("req_01ABC"));
            }
            other => panic!("Expected HttpMeta first, got {other:?}"),
        }
        // Next event is the actual provider frame.
        assert!(matches!(
            stream.next().await,
            Some(Ok(StreamEvent::MessageStop))
        ));
    }

    #[tokio::test]
    async fn test_sse_stream_emits_http_meta_with_none_when_no_header() {
        let inner = bytes_stream(vec![
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ]);
        let mut stream = SseStream::new(inner);
        // `SseStream::new` leaves `request_id: None`; the preamble
        // still fires so consumers can pattern-match unconditionally.
        match stream.next().await {
            Some(Ok(StreamEvent::HttpMeta { request_id })) => {
                assert!(request_id.is_none());
            }
            other => panic!("Expected HttpMeta first, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sse_stream_parses_complete_event() {
        let inner = bytes_stream(vec![
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ]);
        let mut stream = SseStream::new(inner);
        expect_http_meta(&mut stream, None).await;
        let event = stream.next().await;
        assert!(matches!(event, Some(Ok(StreamEvent::MessageStop))));
        // stream should be finished
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_sse_stream_handles_partial_chunks() {
        let inner = bytes_stream(vec![
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n",
            "\n",
        ]);
        let mut stream = SseStream::new(inner);
        expect_http_meta(&mut stream, None).await;
        let event = stream.next().await;
        assert!(matches!(event, Some(Ok(StreamEvent::MessageStop))));
    }

    #[tokio::test]
    async fn test_sse_stream_handles_crlf_delimiters() {
        let inner = bytes_stream(vec![
            "event: message_stop\r\ndata: {\"type\":\"message_stop\"}\r\n\r\n",
        ]);
        let mut stream = SseStream::new(inner);
        expect_http_meta(&mut stream, None).await;
        let event = stream.next().await;
        assert!(matches!(event, Some(Ok(StreamEvent::MessageStop))));
    }

    #[tokio::test]
    async fn test_sse_stream_emits_error_for_malformed_then_continues() {
        let inner = bytes_stream(vec![
            "event: unknown\ndata: {bad json}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ]);
        let mut stream = SseStream::new(inner);
        expect_http_meta(&mut stream, None).await;
        let first = stream.next().await;
        assert!(
            matches!(&first, Some(Ok(StreamEvent::Error { message, .. })) if message.contains("malformed SSE JSON")),
            "expected StreamEvent::Error, got {first:?}"
        );
        // Error from malformed JSON marks finished because StreamEvent::Error sets finished=true
    }

    #[tokio::test]
    async fn test_sse_stream_multiple_events() {
        let inner = bytes_stream(vec![
            "event: ping\ndata: {}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ]);
        let mut stream = SseStream::new(inner);
        expect_http_meta(&mut stream, None).await;
        let first = stream.next().await;
        assert!(matches!(first, Some(Ok(StreamEvent::Ping))));
        let second = stream.next().await;
        assert!(matches!(second, Some(Ok(StreamEvent::MessageStop))));
    }

    #[tokio::test]
    async fn test_sse_stream_empty_input() {
        let inner = bytes_stream(vec![]);
        let mut stream = SseStream::new(inner);
        // Empty upstream body still yields the HttpMeta preamble
        // (consumers rely on it as a deterministic first event), then
        // the stream terminates.
        expect_http_meta(&mut stream, None).await;
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_sse_stream_error_marks_finished() {
        let inner = bytes_stream(vec![
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"message\":\"boom\"}}\n\n",
        ]);
        let mut stream = SseStream::new(inner);
        expect_http_meta(&mut stream, None).await;
        let event = stream.next().await;
        assert!(matches!(event, Some(Ok(StreamEvent::Error { .. }))));
        assert!(stream.next().await.is_none());
    }
}
