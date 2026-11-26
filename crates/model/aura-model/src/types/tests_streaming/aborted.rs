//! `StreamAbortedWithPartial` (per-tool-call streaming retry).

use crate::types::*;

#[test]
fn stream_aborted_with_partial_preserves_tool_use() {
    // Mid-stream SSE error landing AFTER a content_block_start +
    // partial input_json_delta MUST surface the in-flight tool_use
    // inside `StreamAbortedWithPartial`, not get silently promoted
    // into `tool_uses` and treated like a successful call. This is
    // the contract `complete_with_streaming` relies on to drive its
    // per-tool-call retry loop.
    let mut acc = StreamAccumulator::new();
    acc.process(&StreamEvent::MessageStart {
        message_id: "msg_abort".to_string(),
        model: "claude-sonnet".to_string(),
        input_tokens: Some(5),
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    acc.process(&StreamEvent::ContentBlockStart {
        index: 0,
        content_type: StreamContentType::ToolUse {
            id: "toolu_abc".to_string(),
            name: "write_file".to_string(),
        },
    });
    acc.process(&StreamEvent::InputJsonDelta {
        partial_json: r#"{"path":"src/"#.to_string(),
    });
    acc.process(&StreamEvent::InputJsonDelta {
        partial_json: r#"lib.rs","#.to_string(),
    });
    // No ContentBlockStop -- the stream dies here.
    acc.process(&StreamEvent::Error {
        message: "overloaded_error: provider overloaded".to_string(),
        request_id: None,
    });

    let err = acc
        .into_response(0, 100)
        .expect_err("aborted stream must be an error");

    match err {
        crate::ReasonerError::StreamAbortedWithPartial {
            reason,
            partial_tool_use,
        } => {
            assert!(
                reason.contains("overloaded_error"),
                "reason should carry upstream message, got: {reason}"
            );
            let partial = partial_tool_use.expect("partial_tool_use must be Some");
            assert_eq!(partial.tool_use_id, "toolu_abc");
            assert_eq!(partial.tool_name, "write_file");
            assert_eq!(partial.partial_json, "{\"path\":\"src/lib.rs\",");
        }
        other => panic!("expected StreamAbortedWithPartial, got: {other:?}"),
    }
}

#[test]
fn stream_aborted_without_tool_use_returns_transient() {
    // When the SSE error arrives before any `tool_use` content-block
    // started, the no-partial branch must surface as a plain
    // `ReasonerError::Transient` (status 502) so the agent loop's
    // `stream_error_is_retryable` classifier routes the failure
    // through `complete_and_emit_as_deltas` (which has tracing + a
    // proper non-streaming fallback) instead of the silent
    // per-tool-call streaming retry loop in
    // `retry_streaming_for_partial_tool_use`. See the
    // `fix-silent-stream-retry-storm` plan for why this routing
    // matters: without it, a single Anthropic 5xx blip with no tool
    // in flight wedges chat in an invisible 28-second silent-retry
    // storm.
    let mut acc = StreamAccumulator::new();
    acc.process(&StreamEvent::MessageStart {
        message_id: "msg_abort2".to_string(),
        model: "claude-sonnet".to_string(),
        input_tokens: Some(5),
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    acc.process(&StreamEvent::Error {
        message: "api_error: internal server error".to_string(),
        request_id: None,
    });

    let err = acc
        .into_response(0, 100)
        .expect_err("aborted stream must be an error");

    match err {
        crate::ReasonerError::Transient {
            status,
            message,
            retry_after,
        } => {
            assert_eq!(status, 502, "expected synthetic 502 for stream-error abort");
            assert!(
                message.contains("api_error"),
                "message should carry upstream error type, got: {message}"
            );
            assert!(
                message.contains("stream terminated with error"),
                "message should carry the canonical prefix, got: {message}"
            );
            assert!(retry_after.is_none());
        }
        other => panic!("expected Transient, got: {other:?}"),
    }
}
