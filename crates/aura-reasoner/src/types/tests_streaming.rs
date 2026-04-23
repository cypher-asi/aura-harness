use super::*;

// ========================================================================
// StreamAccumulator Tests
// ========================================================================

#[test]
fn test_stream_accumulator_text_only() {
    let mut acc = StreamAccumulator::new();

    acc.process(&StreamEvent::MessageStart {
        message_id: "msg1".to_string(),
        model: "claude".to_string(),
        input_tokens: None,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    acc.process(&StreamEvent::ContentBlockStart {
        index: 0,
        content_type: StreamContentType::Text,
    });
    acc.process(&StreamEvent::TextDelta {
        text: "Hello ".to_string(),
    });
    acc.process(&StreamEvent::TextDelta {
        text: "world!".to_string(),
    });
    acc.process(&StreamEvent::ContentBlockStop { index: 0 });
    acc.process(&StreamEvent::MessageDelta {
        stop_reason: Some(StopReason::EndTurn),
        output_tokens: 10,
    });
    acc.process(&StreamEvent::MessageStop);

    assert_eq!(acc.message_id, "msg1");
    assert_eq!(acc.model, "claude");
    assert_eq!(acc.text_content, "Hello world!");
    assert_eq!(acc.output_tokens, 10);
    assert_eq!(acc.stop_reason, Some(StopReason::EndTurn));
}

#[test]
fn test_stream_accumulator_tool_use() {
    let mut acc = StreamAccumulator::new();

    acc.process(&StreamEvent::MessageStart {
        message_id: "msg1".to_string(),
        model: "claude".to_string(),
        input_tokens: None,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    acc.process(&StreamEvent::ContentBlockStart {
        index: 0,
        content_type: StreamContentType::ToolUse {
            id: "tool1".to_string(),
            name: "read_file".to_string(),
        },
    });
    acc.process(&StreamEvent::InputJsonDelta {
        partial_json: r#"{"path":"#.to_string(),
    });
    acc.process(&StreamEvent::InputJsonDelta {
        partial_json: r#""test.txt"}"#.to_string(),
    });
    acc.process(&StreamEvent::ContentBlockStop { index: 0 });
    acc.process(&StreamEvent::MessageDelta {
        stop_reason: Some(StopReason::ToolUse),
        output_tokens: 20,
    });

    assert_eq!(acc.tool_uses.len(), 1);
    assert_eq!(acc.tool_uses[0].id, "tool1");
    assert_eq!(acc.tool_uses[0].name, "read_file");
    assert_eq!(acc.tool_uses[0].input_json, r#"{"path":"test.txt"}"#);
}

#[test]
fn test_stream_accumulator_thinking() {
    let mut acc = StreamAccumulator::new();

    acc.process(&StreamEvent::ContentBlockStart {
        index: 0,
        content_type: StreamContentType::Thinking,
    });
    acc.process(&StreamEvent::ThinkingDelta {
        thinking: "Let me ".to_string(),
    });
    acc.process(&StreamEvent::ThinkingDelta {
        thinking: "think...".to_string(),
    });
    acc.process(&StreamEvent::SignatureDelta {
        signature: "sig_abc".to_string(),
    });
    acc.process(&StreamEvent::ContentBlockStop { index: 0 });

    assert_eq!(acc.thinking_content, "Let me think...");
    assert_eq!(acc.thinking_signature, Some("sig_abc".to_string()));
}

#[test]
fn test_stream_accumulator_mixed_content() {
    let mut acc = StreamAccumulator::new();

    acc.process(&StreamEvent::ContentBlockStart {
        index: 0,
        content_type: StreamContentType::Thinking,
    });
    acc.process(&StreamEvent::ThinkingDelta {
        thinking: "Thinking...".to_string(),
    });
    acc.process(&StreamEvent::ContentBlockStop { index: 0 });

    acc.process(&StreamEvent::ContentBlockStart {
        index: 1,
        content_type: StreamContentType::Text,
    });
    acc.process(&StreamEvent::TextDelta {
        text: "Response text".to_string(),
    });
    acc.process(&StreamEvent::ContentBlockStop { index: 1 });

    acc.process(&StreamEvent::ContentBlockStart {
        index: 2,
        content_type: StreamContentType::ToolUse {
            id: "tool1".to_string(),
            name: "list_files".to_string(),
        },
    });
    acc.process(&StreamEvent::InputJsonDelta {
        partial_json: r#"{"path":"."}"#.to_string(),
    });
    acc.process(&StreamEvent::ContentBlockStop { index: 2 });

    assert_eq!(acc.thinking_content, "Thinking...");
    assert_eq!(acc.text_content, "Response text");
    assert_eq!(acc.tool_uses.len(), 1);
}

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
fn test_stream_accumulator_invalid_json_handling() {
    let mut acc = StreamAccumulator::new();

    acc.tool_uses.push(AccumulatedToolUse {
        id: "tool1".to_string(),
        name: "test".to_string(),
        input_json: "invalid json {{{".to_string(),
    });

    let response = acc.into_response(0, 0).unwrap();

    if let ContentBlock::ToolUse { input, .. } = &response.message.content[0] {
        assert!(input.get("raw").is_some());
    } else {
        panic!("Expected ToolUse block");
    }
}

#[test]
fn test_stream_accumulator_empty_tool_json() {
    let mut acc = StreamAccumulator::new();

    acc.tool_uses.push(AccumulatedToolUse {
        id: "tool1".to_string(),
        name: "test".to_string(),
        input_json: String::new(),
    });

    let response = acc.into_response(0, 0).unwrap();

    if let ContentBlock::ToolUse { input, .. } = &response.message.content[0] {
        assert!(input.is_object());
        assert!(input.as_object().unwrap().is_empty());
    } else {
        panic!("Expected ToolUse block");
    }
}

#[test]
fn test_stream_accumulator_multiple_tools() {
    let mut acc = StreamAccumulator::new();

    acc.process(&StreamEvent::ContentBlockStart {
        index: 0,
        content_type: StreamContentType::ToolUse {
            id: "tool1".to_string(),
            name: "list_files".to_string(),
        },
    });
    acc.process(&StreamEvent::InputJsonDelta {
        partial_json: r#"{"path":"."}"#.to_string(),
    });
    acc.process(&StreamEvent::ContentBlockStop { index: 0 });

    acc.process(&StreamEvent::ContentBlockStart {
        index: 1,
        content_type: StreamContentType::ToolUse {
            id: "tool2".to_string(),
            name: "read_file".to_string(),
        },
    });
    acc.process(&StreamEvent::InputJsonDelta {
        partial_json: r#"{"path":"file.txt"}"#.to_string(),
    });
    acc.process(&StreamEvent::ContentBlockStop { index: 1 });

    assert_eq!(acc.tool_uses.len(), 2);
    assert_eq!(acc.tool_uses[0].name, "list_files");
    assert_eq!(acc.tool_uses[1].name, "read_file");
}

#[test]
fn test_stream_accumulator_ping_and_error() {
    let mut acc = StreamAccumulator::new();

    acc.process(&StreamEvent::Ping);
    acc.process(&StreamEvent::Error {
        message: "test error".to_string(),
    });

    assert!(acc.text_content.is_empty());
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
    });

    let err = acc.into_response(0, 100).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("stream terminated with error"),
        "expected stream-termination prefix, got: {msg}"
    );
    // Model and message_id should be threaded into the error so
    // operators can correlate with provider / router logs.
    assert!(msg.contains("model=claude-sonnet-test"), "missing model: {msg}");
    assert!(msg.contains("msg_id=msg_01ABC"), "missing msg_id: {msg}");
    assert!(msg.contains("Internal server error"), "missing raw msg: {msg}");
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
    });

    let err = acc.into_response(0, 100).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("stream terminated with error"),
        "partial-content path must still fail, got: {msg}"
    );
    assert!(msg.contains("overloaded_error"), "error type preserved: {msg}");
}

#[test]
fn test_stream_accumulator_into_response_error_without_message_start() {
    // If the SSE error arrives before `message_start` we still fail
    // cleanly and just omit the context fragment (no trailing empty
    // parentheses in the rendered message).
    let mut acc = StreamAccumulator::new();
    acc.process(&StreamEvent::Error {
        message: "connection reset by peer".to_string(),
    });

    let err = acc.into_response(0, 100).unwrap_err();
    let msg = err.to_string();
    assert_eq!(
        msg,
        "stream terminated with error: connection reset by peer",
        "no empty context parentheses when metadata unavailable"
    );
}

#[test]
fn test_provider_trace() {
    let trace = ProviderTrace::new("claude", 500).with_request_id("req123");

    assert_eq!(trace.model, "claude");
    assert_eq!(trace.latency_ms, 500);
    assert_eq!(trace.request_id, Some("req123".to_string()));
}

#[test]
fn test_usage_with_cache_tokens() {
    let usage = Usage::new(100, 50).with_cache(Some(80), Some(20));
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.output_tokens, 50);
    assert_eq!(usage.cache_creation_input_tokens, Some(80));
    assert_eq!(usage.cache_read_input_tokens, Some(20));
    assert_eq!(usage.total(), 150);
}

#[test]
fn test_usage_default_has_no_cache() {
    let usage = Usage::default();
    assert_eq!(usage.cache_creation_input_tokens, None);
    assert_eq!(usage.cache_read_input_tokens, None);
}

#[test]
fn test_stream_accumulator_input_tokens_from_message_start() {
    let mut acc = StreamAccumulator::new();

    acc.process(&StreamEvent::MessageStart {
        message_id: "msg_cache".to_string(),
        model: "claude".to_string(),
        input_tokens: Some(200),
        cache_creation_input_tokens: Some(150),
        cache_read_input_tokens: Some(50),
    });
    acc.process(&StreamEvent::ContentBlockStart {
        index: 0,
        content_type: StreamContentType::Text,
    });
    acc.process(&StreamEvent::TextDelta {
        text: "Cached!".to_string(),
    });
    acc.process(&StreamEvent::ContentBlockStop { index: 0 });
    acc.process(&StreamEvent::MessageDelta {
        stop_reason: Some(StopReason::EndTurn),
        output_tokens: 3,
    });

    let response = acc.into_response(0, 100).unwrap();

    assert_eq!(response.usage.input_tokens, 200);
    assert_eq!(response.usage.cache_creation_input_tokens, Some(150));
    assert_eq!(response.usage.cache_read_input_tokens, Some(50));
    assert_eq!(response.model_used, "claude");
}

#[test]
fn test_stream_accumulator_interleaved_text_tool_text() {
    let mut acc = StreamAccumulator::new();

    acc.process(&StreamEvent::ContentBlockStart {
        index: 0,
        content_type: StreamContentType::Text,
    });
    acc.process(&StreamEvent::TextDelta {
        text: "Before tool. ".to_string(),
    });
    acc.process(&StreamEvent::ContentBlockStop { index: 0 });

    acc.process(&StreamEvent::ContentBlockStart {
        index: 1,
        content_type: StreamContentType::ToolUse {
            id: "t1".to_string(),
            name: "read_file".to_string(),
        },
    });
    acc.process(&StreamEvent::InputJsonDelta {
        partial_json: r#"{"path":"a.txt"}"#.to_string(),
    });
    acc.process(&StreamEvent::ContentBlockStop { index: 1 });

    acc.process(&StreamEvent::ContentBlockStart {
        index: 2,
        content_type: StreamContentType::ToolUse {
            id: "t2".to_string(),
            name: "list_files".to_string(),
        },
    });
    acc.process(&StreamEvent::InputJsonDelta {
        partial_json: r#"{"path":"."}"#.to_string(),
    });
    acc.process(&StreamEvent::ContentBlockStop { index: 2 });

    assert_eq!(acc.text_content, "Before tool. ");
    assert_eq!(acc.tool_uses.len(), 2);
    assert_eq!(acc.tool_uses[0].id, "t1");
    assert_eq!(acc.tool_uses[1].id, "t2");
}

#[test]
fn test_stream_accumulator_no_events() {
    let acc = StreamAccumulator::new();
    assert!(acc.message_id.is_empty());
    assert!(acc.text_content.is_empty());
    assert!(acc.tool_uses.is_empty());
    assert!(acc.stop_reason.is_none());
    assert_eq!(acc.output_tokens, 0);
}

#[test]
fn test_stream_accumulator_finalize_uses_fallback_input_tokens() {
    let acc = StreamAccumulator::new();
    let response = acc.into_response(999, 50).unwrap();
    assert_eq!(response.usage.input_tokens, 999);
    assert_eq!(response.stop_reason, StopReason::EndTurn);
    assert!(response.message.content.is_empty());
}

#[test]
fn test_stream_accumulator_signature_appends() {
    let mut acc = StreamAccumulator::new();
    acc.process(&StreamEvent::ContentBlockStart {
        index: 0,
        content_type: StreamContentType::Thinking,
    });
    acc.process(&StreamEvent::SignatureDelta {
        signature: "part1".to_string(),
    });
    acc.process(&StreamEvent::SignatureDelta {
        signature: "part2".to_string(),
    });
    acc.process(&StreamEvent::ContentBlockStop { index: 0 });
    assert_eq!(acc.thinking_signature, Some("part1part2".to_string()));
}

// ========================================================================
// ContentBlock — serialization round-trips
// ========================================================================

#[test]
fn test_content_block_thinking_serialization_round_trip() {
    let block = ContentBlock::Thinking {
        thinking: "hmm".to_string(),
        signature: Some("sig".to_string()),
    };
    let json = serde_json::to_string(&block).unwrap();
    let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
    match parsed {
        ContentBlock::Thinking {
            thinking,
            signature,
        } => {
            assert_eq!(thinking, "hmm");
            assert_eq!(signature, Some("sig".to_string()));
        }
        _ => panic!("Expected Thinking"),
    }
}

#[test]
fn test_content_block_thinking_without_signature_skips_field() {
    let block = ContentBlock::Thinking {
        thinking: "hmm".to_string(),
        signature: None,
    };
    let json = serde_json::to_string(&block).unwrap();
    assert!(!json.contains("signature"));
}

#[test]
fn test_content_block_tool_use_serialization_round_trip() {
    let block = ContentBlock::tool_use("id1", "run_command", serde_json::json!({"cmd": "ls"}));
    let json = serde_json::to_string(&block).unwrap();
    let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
    match parsed {
        ContentBlock::ToolUse { id, name, input } => {
            assert_eq!(id, "id1");
            assert_eq!(name, "run_command");
            assert_eq!(input["cmd"], "ls");
        }
        _ => panic!("Expected ToolUse"),
    }
}

#[test]
fn test_content_block_tool_result_serialization() {
    let block =
        ContentBlock::tool_result("tu_1", ToolResultContent::text("file contents here"), false);
    let json = serde_json::to_string(&block).unwrap();
    assert!(json.contains("tool_result"));
    assert!(json.contains("tu_1"));
}

// ========================================================================
// ModelRequest builder — additional edge cases
// ========================================================================

#[test]
fn test_model_request_builder_with_thinking() {
    use super::request::ThinkingConfig;

    let request = ModelRequest::builder("model", "system")
        .thinking(ThinkingConfig {
            budget_tokens: 4096,
        })
        .build();

    assert!(request.thinking.is_some());
    assert_eq!(request.thinking.unwrap().budget_tokens, 4096);
}

#[test]
fn test_model_request_builder_with_auth_token() {
    let request = ModelRequest::builder("model", "system")
        .auth_token(Some("tok_abc".to_string()))
        .build();

    assert_eq!(request.auth_token, Some("tok_abc".to_string()));
}

#[test]
fn test_model_request_builder_multiple_messages() {
    let request = ModelRequest::builder("model", "system")
        .message(Message::user("first"))
        .message(Message::assistant("response"))
        .message(Message::user("second"))
        .build();

    assert_eq!(request.messages.len(), 3);
    assert_eq!(request.messages[0].role, Role::User);
    assert_eq!(request.messages[1].role, Role::Assistant);
    assert_eq!(request.messages[2].role, Role::User);
}

// ========================================================================
// StopReason serialization
// ========================================================================

#[test]
fn test_stop_reason_serialization() {
    let reasons = [
        (StopReason::EndTurn, "\"end_turn\""),
        (StopReason::ToolUse, "\"tool_use\""),
        (StopReason::MaxTokens, "\"max_tokens\""),
        (StopReason::StopSequence, "\"stop_sequence\""),
    ];
    for (reason, expected) in reasons {
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, expected);
        let parsed: StopReason = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, reason);
    }
}

// ========================================================================
// Role serialization
// ========================================================================

#[test]
fn test_role_serialization() {
    assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
    assert_eq!(
        serde_json::to_string(&Role::Assistant).unwrap(),
        "\"assistant\""
    );
}
