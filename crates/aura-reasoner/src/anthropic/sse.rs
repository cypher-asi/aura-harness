use super::api_types::{SseContentBlock, SseDelta, SseEvent};
use crate::error::ReasonerError;
use crate::{StopReason, StreamContentType, StreamEvent};
use futures_util::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};
use tracing::trace;

const MAX_SSE_BUFFER_SIZE: usize = 10 * 1024 * 1024;

/// A stream that parses SSE events from an HTTP byte stream.
pub(super) struct SseStream<S> {
    inner: S,
    buffer: String,
    finished: bool,
}

impl<S> SseStream<S> {
    pub(super) const fn new(inner: S) -> Self {
        Self {
            inner,
            buffer: String::new(),
            finished: false,
        }
    }
}

impl<S, E> Stream for SseStream<S>
where
    S: Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    E: std::error::Error + Send + Sync + 'static,
{
    type Item = anyhow::Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.finished {
            return Poll::Ready(None);
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
                    if let Ok(s) = std::str::from_utf8(&bytes) {
                        self.buffer.push_str(s);
                    }
                    if self.buffer.len() > MAX_SSE_BUFFER_SIZE {
                        self.finished = true;
                        return Poll::Ready(Some(Err(ReasonerError::Internal(format!(
                            "SSE buffer exceeded {MAX_SSE_BUFFER_SIZE} bytes"
                        ))
                        .into())));
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    self.finished = true;
                    return Poll::Ready(Some(Err(ReasonerError::Request(format!(
                        "Stream error: {e}"
                    ))
                    .into())));
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
            trace!(data = %data, error = %e, "Failed to parse SSE event");
            return None;
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
        SseEvent::Error { error } => Some(StreamEvent::Error {
            message: error.message,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    fn bytes_stream(
        chunks: Vec<&str>,
    ) -> impl Stream<Item = Result<bytes::Bytes, std::io::Error>> + Unpin {
        futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|c| Ok(bytes::Bytes::from(c.to_string())))
                .collect::<Vec<_>>(),
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
    fn test_parse_event_with_invalid_json_returns_none() {
        let event = parse_sse_event("event: message_start\ndata: {not valid json!!}");
        assert!(event.is_none());
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
            Some(StreamEvent::Error { message }) => assert_eq!(message, "overloaded"),
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

    #[tokio::test]
    async fn test_sse_stream_parses_complete_event() {
        let inner = bytes_stream(vec![
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ]);
        let mut stream = SseStream::new(inner);
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
        let event = stream.next().await;
        assert!(matches!(event, Some(Ok(StreamEvent::MessageStop))));
    }

    #[tokio::test]
    async fn test_sse_stream_handles_crlf_delimiters() {
        let inner = bytes_stream(vec![
            "event: message_stop\r\ndata: {\"type\":\"message_stop\"}\r\n\r\n",
        ]);
        let mut stream = SseStream::new(inner);
        let event = stream.next().await;
        assert!(matches!(event, Some(Ok(StreamEvent::MessageStop))));
    }

    #[tokio::test]
    async fn test_sse_stream_skips_malformed_then_continues() {
        // Events must arrive in separate chunks so the stream re-polls and
        // picks up the valid event after the malformed one drains the buffer.
        let inner = bytes_stream(vec![
            "event: unknown\ndata: {bad json}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ]);
        let mut stream = SseStream::new(inner);
        let event = stream.next().await;
        assert!(matches!(event, Some(Ok(StreamEvent::MessageStop))));
    }

    #[tokio::test]
    async fn test_sse_stream_multiple_events() {
        let inner = bytes_stream(vec![
            "event: ping\ndata: {}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ]);
        let mut stream = SseStream::new(inner);
        let first = stream.next().await;
        assert!(matches!(first, Some(Ok(StreamEvent::Ping))));
        let second = stream.next().await;
        assert!(matches!(second, Some(Ok(StreamEvent::MessageStop))));
    }

    #[tokio::test]
    async fn test_sse_stream_empty_input() {
        let inner = bytes_stream(vec![]);
        let mut stream = SseStream::new(inner);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_sse_stream_error_marks_finished() {
        let inner = bytes_stream(vec![
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"message\":\"boom\"}}\n\n",
        ]);
        let mut stream = SseStream::new(inner);
        let event = stream.next().await;
        assert!(matches!(event, Some(Ok(StreamEvent::Error { .. }))));
        assert!(stream.next().await.is_none());
    }
}
