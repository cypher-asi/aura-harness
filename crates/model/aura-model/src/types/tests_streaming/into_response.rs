//! `StreamAccumulator::into_response`: responses, traces and stream errors.

use crate::types::*;

#[test]
fn test_stream_accumulator_into_response() {
    let mut acc = StreamAccumulator::new();

    acc.process(&StreamEvent::MessageStart {
        message_id: "msg123".to_string(),
        model: "claude-opus-4-6".to_string(),
        input_tokens: None,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    acc.process(&StreamEvent::ContentBlockStart {
        index: 0,
        content_type: StreamContentType::Text,
    });
    acc.process(&StreamEvent::TextDelta {
        text: "Hello!".to_string(),
    });
    acc.process(&StreamEvent::ContentBlockStop { index: 0 });
    acc.process(&StreamEvent::MessageDelta {
        stop_reason: Some(StopReason::EndTurn),
        output_tokens: 5,
        input_tokens: None,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });

    let response = acc.into_response(100, 500).unwrap();

    assert_eq!(response.stop_reason, StopReason::EndTurn);
    assert_eq!(response.message.text_content(), "Hello!");
    assert_eq!(response.usage.input_tokens, 100);
    assert_eq!(response.usage.output_tokens, 5);
    assert_eq!(response.trace.model, "claude-opus-4-6");
    assert_eq!(response.trace.latency_ms, 500);
}

#[test]
fn test_stream_accumulator_into_response_with_thinking() {
    let mut acc = StreamAccumulator::new();

    acc.thinking_content = "Deep thoughts...".to_string();
    acc.thinking_signature = Some("sig123".to_string());
    acc.text_content = "Here's my answer".to_string();
    acc.stop_reason = Some(StopReason::EndTurn);
    acc.model = "claude".to_string();

    let response = acc.into_response(50, 200).unwrap();

    assert_eq!(response.message.content.len(), 2);
    assert!(matches!(
        &response.message.content[0],
        ContentBlock::Thinking { .. }
    ));
    assert!(matches!(
        &response.message.content[1],
        ContentBlock::Text { .. }
    ));
}

#[test]
fn test_stream_accumulator_into_response_with_tool() {
    let mut acc = StreamAccumulator::new();

    acc.tool_uses.push(AccumulatedToolUse {
        id: "tool1".to_string(),
        name: "read_file".to_string(),
        input_json: r#"{"path":"test.txt"}"#.to_string(),
    });
    acc.stop_reason = Some(StopReason::ToolUse);

    let response = acc.into_response(50, 100).unwrap();

    assert_eq!(response.stop_reason, StopReason::ToolUse);
    assert!(response.message.has_tool_use());

    if let ContentBlock::ToolUse { id, name, input } = &response.message.content[0] {
        assert_eq!(id, "tool1");
        assert_eq!(name, "read_file");
        assert_eq!(input["path"], "test.txt");
    } else {
        panic!("Expected ToolUse block");
    }
}

#[test]
fn test_stream_accumulator_captures_provider_request_id_from_http_meta() {
    // The synthetic `HttpMeta` preamble carries the HTTP-header
    // request id. The accumulator must adopt it so a subsequent
    // `MessageStart` (which carries only the Anthropic *message* id)
    // doesn't clobber the HTTP value.
    let mut acc = StreamAccumulator::new();
    acc.process(&StreamEvent::HttpMeta {
        request_id: Some("req_01XYZ".to_string()),
        plan_usage: Vec::new(),
    });
    acc.process(&StreamEvent::MessageStart {
        message_id: "msg_01ABC".to_string(),
        model: "claude-sonnet-4".to_string(),
        input_tokens: None,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    assert_eq!(acc.provider_request_id.as_deref(), Some("req_01XYZ"));
    assert_eq!(acc.message_id, "msg_01ABC");
}

#[test]
fn test_stream_accumulator_http_meta_with_none_is_noop() {
    // `SseStream` always emits the `HttpMeta` preamble, even when no
    // header was captured. A `None` value must not overwrite a later
    // body-level fallback.
    let mut acc = StreamAccumulator::new();
    acc.process(&StreamEvent::HttpMeta {
        request_id: None,
        plan_usage: Vec::new(),
    });
    acc.process(&StreamEvent::Error {
        message: "api_error: Internal server error".to_string(),
        request_id: Some("req_01FALLBACK".to_string()),
    });
    assert_eq!(
        acc.provider_request_id.as_deref(),
        Some("req_01FALLBACK"),
        "body-level request_id should be adopted when HTTP header was absent"
    );
}

#[test]
fn test_stream_accumulator_http_meta_wins_over_body_fallback() {
    // When both the header *and* the body carry a request_id, the
    // header value is authoritative (it covers the success path too
    // and is the one provider / router logs actually key on).
    let mut acc = StreamAccumulator::new();
    acc.process(&StreamEvent::HttpMeta {
        request_id: Some("req_01HEADER".to_string()),
        plan_usage: Vec::new(),
    });
    acc.process(&StreamEvent::Error {
        message: "api_error: Internal server error".to_string(),
        request_id: Some("req_01BODY".to_string()),
    });
    assert_eq!(acc.provider_request_id.as_deref(), Some("req_01HEADER"));
}

#[test]
fn test_stream_accumulator_into_response_propagates_error_with_no_content() {
    let mut acc = StreamAccumulator::new();
    acc.process(&StreamEvent::MessageStart {
        message_id: "msg_01ABC".to_string(),
        model: "claude-sonnet-test".to_string(),
        input_tokens: None,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    acc.process(&StreamEvent::Error {
        message: "Internal server error".to_string(),
        request_id: None,
    });

    let err = acc.into_response(0, 100).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("stream terminated with error"),
        "expected stream-termination prefix, got: {msg}"
    );
    // Model and message_id should be threaded into the error so
    // operators can correlate with provider / router logs.
    assert!(
        msg.contains("model=claude-sonnet-test"),
        "missing model: {msg}"
    );
    assert!(msg.contains("msg_id=msg_01ABC"), "missing msg_id: {msg}");
    assert!(
        msg.contains("Internal server error"),
        "missing raw msg: {msg}"
    );
    // No HttpMeta was processed, so the error should NOT claim a
    // request_id — better to omit the key than to surface a
    // misleading `request_id=` fragment.
    assert!(
        !msg.contains("request_id="),
        "should not fabricate request_id when absent, got: {msg}"
    );
}

#[test]
fn test_stream_accumulator_into_response_error_includes_request_id() {
    // Golden for the F1 plumbing: when the transport layer captured a
    // response-header `x-request-id` via `HttpMeta`, the error string
    // must surface it alongside `model=…, msg_id=…` so the operator
    // can grep provider / router logs directly.
    let mut acc = StreamAccumulator::new();
    acc.process(&StreamEvent::HttpMeta {
        request_id: Some("req_01XYZ".to_string()),
        plan_usage: Vec::new(),
    });
    acc.process(&StreamEvent::MessageStart {
        message_id: "msg_01ABC".to_string(),
        model: "claude-sonnet-4".to_string(),
        input_tokens: None,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    acc.process(&StreamEvent::Error {
        message: "api_error: Internal server error".to_string(),
        request_id: None,
    });

    let err = acc.into_response(0, 100).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("stream terminated with error"), "got: {msg}");
    assert!(msg.contains("model=claude-sonnet-4"), "got: {msg}");
    assert!(msg.contains("msg_id=msg_01ABC"), "got: {msg}");
    assert!(msg.contains("request_id=req_01XYZ"), "got: {msg}");
    assert!(
        msg.contains("api_error: Internal server error"),
        "got: {msg}"
    );
}

#[test]
fn test_stream_accumulator_into_response_populates_trace_split_ids() {
    // Happy path: `into_response` must populate both `message_id` and
    // `provider_request_id` on `ProviderTrace` when the stream
    // finished cleanly, so downstream observers
    // (`emit_debug_llm_call`, persistence of `llm_calls.jsonl`) get
    // both ids.
    let mut acc = StreamAccumulator::new();
    acc.process(&StreamEvent::HttpMeta {
        request_id: Some("req_01HAPPY".to_string()),
        plan_usage: Vec::new(),
    });
    acc.process(&StreamEvent::MessageStart {
        message_id: "msg_01HAPPY".to_string(),
        model: "claude-sonnet-4".to_string(),
        input_tokens: Some(10),
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    acc.process(&StreamEvent::MessageStop);
    let response = acc.into_response(10, 42).expect("happy path");
    assert_eq!(response.trace.message_id.as_deref(), Some("msg_01HAPPY"));
    assert_eq!(
        response.trace.provider_request_id.as_deref(),
        Some("req_01HAPPY")
    );
    assert_eq!(response.trace.model, "claude-sonnet-4");
}

#[test]
fn test_stream_accumulator_into_response_propagates_error_even_with_partial_content() {
    // Regression: previously `into_response` silently dropped a
    // mid-stream error if any text or tool_use content had been
    // accumulated. That caused partial tool_use blocks to be executed
    // as if the stream had finished cleanly. The fix always
    // propagates `stream_error`.
    let mut acc = StreamAccumulator::new();
    acc.process(&StreamEvent::MessageStart {
        message_id: "msg_partial".to_string(),
        model: "claude-sonnet-test".to_string(),
        input_tokens: None,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    acc.process(&StreamEvent::ContentBlockStart {
        index: 0,
        content_type: StreamContentType::Text,
    });
    acc.process(&StreamEvent::TextDelta {
        text: "Partial response before upstream blew up".to_string(),
    });
    acc.process(&StreamEvent::Error {
        message: "overloaded_error: service is overloaded".to_string(),
        request_id: None,
    });

    let err = acc.into_response(0, 100).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("stream terminated with error"),
        "partial-content path must still fail, got: {msg}"
    );
    assert!(
        msg.contains("overloaded_error"),
        "error type preserved: {msg}"
    );
}

#[test]
fn test_stream_accumulator_into_response_error_without_message_start() {
    // If the SSE error arrives before `message_start` we still fail
    // cleanly and just omit the context fragment (no trailing empty
    // parentheses in the rendered reason). With no in-flight tool,
    // the variant is `ReasonerError::Transient` (status 502) -- so
    // the surface error message is wrapped with the standard API-error
    // prefix from the `Display` impl. The inner `reason` must still
    // omit the empty parens.
    let mut acc = StreamAccumulator::new();
    acc.process(&StreamEvent::Error {
        message: "connection reset by peer".to_string(),
        request_id: None,
    });

    let err = acc.into_response(0, 100).unwrap_err();
    match err {
        crate::ReasonerError::Transient {
            status, message, ..
        } => {
            assert_eq!(status, 502);
            assert_eq!(
                message, "stream terminated with error: connection reset by peer",
                "no empty context parentheses when metadata unavailable"
            );
        }
        other => panic!("expected Transient, got: {other:?}"),
    }
}
