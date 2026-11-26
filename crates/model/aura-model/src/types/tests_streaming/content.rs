//! `StreamAccumulator`: assembling text, tool-use and thinking blocks.

use crate::types::*;

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
        input_tokens: None,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
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
        input_tokens: None,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
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
        request_id: None,
    });

    assert!(acc.text_content.is_empty());
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
