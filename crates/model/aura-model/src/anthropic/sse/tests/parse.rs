use super::*;

#[tokio::test]
async fn idle_stream_times_out_and_then_terminates() {
    let inner = futures_util::stream::pending::<Result<bytes::Bytes, std::io::Error>>();
    let mut stream = SseStream::with_request_id(inner, None)
        .with_idle_timeout(std::time::Duration::from_millis(20));
    assert!(matches!(
        stream.next().await,
        Some(Ok(StreamEvent::HttpMeta { .. }))
    ));
    assert!(matches!(
        stream.next().await,
        Some(Err(crate::ReasonerError::Timeout))
    ));
    assert!(stream.next().await.is_none());
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
fn redacted_thinking_block_start_parses_as_thinking() {
    // OpenRouter's Anthropic endpoint opens a `redacted_thinking`
    // block for reasoning models whose reasoning is encrypted.
    let event = parse_sse_event(
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"redacted_thinking\",\"data\":\"gAAAAB\"}}",
    );
    assert!(
        matches!(
            event,
            Some(StreamEvent::ContentBlockStart {
                index: 0,
                content_type: crate::StreamContentType::Thinking,
            })
        ),
        "got {event:?}"
    );
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
                ..
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

#[test]
fn test_parse_deepseek_usage_aliases() {
    let event = parse_sse_event(
        r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"deepseek-v4-flash","usage":{"prompt_tokens":100,"prompt_cache_miss_tokens":80,"prompt_cache_hit_tokens":20}}}"#,
    );
    assert!(matches!(
        event,
        Some(StreamEvent::MessageStart {
            input_tokens: Some(100),
            cache_creation_input_tokens: Some(80),
            cache_read_input_tokens: Some(20),
            ..
        })
    ));

    let event = parse_sse_event(
        r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"completion_tokens":25}}"#,
    );
    assert!(matches!(
        event,
        Some(StreamEvent::MessageDelta {
            output_tokens: 25,
            ..
        })
    ));
}
