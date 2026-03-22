//! Normalized provider-agnostic conversation types.
//!
//! These are AURA canonical types for model interactions.
//! Every provider adapter maps to/from these types.

mod content;
mod message;
mod request;
mod response;
mod streaming;
mod tool;

pub use content::{ContentBlock, ImageSource, Role, ToolResultContent};
pub use message::Message;
pub use request::{ModelRequest, ModelRequestBuilder, ThinkingConfig};
pub use response::{ModelResponse, ProviderTrace, StopReason, Usage};
pub use streaming::{AccumulatedToolUse, StreamAccumulator, StreamContentType, StreamEvent};
pub use tool::{CacheControl, ToolChoice, ToolDefinition};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_user() {
        let msg = Message::user("Hello");
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.text_content(), "Hello");
    }

    #[test]
    fn test_message_assistant() {
        let msg = Message::assistant("Hi there");
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.text_content(), "Hi there");
    }

    #[test]
    fn test_message_tool_results() {
        let results = vec![
            ("id1".to_string(), ToolResultContent::text("result1"), false),
            ("id2".to_string(), ToolResultContent::text("error"), true),
        ];
        let msg = Message::tool_results(results);
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.len(), 2);
    }

    #[test]
    fn test_tool_definition() {
        let tool = ToolDefinition::new(
            "fs.read",
            "Read a file",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        );
        assert_eq!(tool.name, "fs.read");
    }

    #[test]
    fn test_model_request_builder() {
        let request = ModelRequest::builder("claude-opus-4-6", "You are helpful")
            .message(Message::user("Hi"))
            .max_tokens(1000)
            .temperature(0.7)
            .build();

        assert_eq!(request.model, "claude-opus-4-6");
        assert_eq!(request.system, "You are helpful");
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.max_tokens, 1000);
        assert_eq!(request.temperature, Some(0.7));
    }

    #[test]
    fn test_content_block_serialization() {
        let text = ContentBlock::text("Hello");
        let json = serde_json::to_string(&text).unwrap();
        assert!(json.contains("\"type\":\"text\""));

        let tool_use =
            ContentBlock::tool_use("123", "fs.read", serde_json::json!({"path": "test"}));
        let json = serde_json::to_string(&tool_use).unwrap();
        assert!(json.contains("\"type\":\"tool_use\""));
    }

    #[test]
    fn test_usage() {
        let usage = Usage::new(100, 50);
        assert_eq!(usage.total(), 150);
    }

    // ========================================================================
    // Message Tests
    // ========================================================================

    #[test]
    fn test_message_with_multiple_content_blocks() {
        let msg = Message::new(
            Role::Assistant,
            vec![
                ContentBlock::text("Let me help you."),
                ContentBlock::tool_use("tool1", "fs_read", serde_json::json!({"path": "test.txt"})),
            ],
        );

        assert!(msg.has_tool_use());
        assert_eq!(msg.tool_uses().len(), 1);
        assert_eq!(msg.text_content(), "Let me help you.");
    }

    #[test]
    fn test_message_text_content_concatenation() {
        let msg = Message::new(
            Role::Assistant,
            vec![ContentBlock::text("Hello "), ContentBlock::text("world!")],
        );

        assert_eq!(msg.text_content(), "Hello world!");
    }

    #[test]
    fn test_message_no_tool_use() {
        let msg = Message::assistant("Just text");
        assert!(!msg.has_tool_use());
        assert!(msg.tool_uses().is_empty());
    }

    // ========================================================================
    // ContentBlock Tests
    // ========================================================================

    #[test]
    fn test_content_block_as_text() {
        let text = ContentBlock::text("hello");
        assert_eq!(text.as_text(), Some("hello"));

        let tool_use = ContentBlock::tool_use("id", "name", serde_json::json!({}));
        assert_eq!(tool_use.as_text(), None);
    }

    #[test]
    fn test_content_block_tool_result() {
        let result =
            ContentBlock::tool_result("tool123", ToolResultContent::text("success"), false);

        match result {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool123");
                assert!(!is_error);
                if let ToolResultContent::Text(t) = content {
                    assert_eq!(t, "success");
                } else {
                    panic!("Expected Text content");
                }
            }
            _ => panic!("Expected ToolResult"),
        }
    }

    #[test]
    fn test_thinking_block() {
        let thinking = ContentBlock::Thinking {
            thinking: "Let me think about this...".to_string(),
            signature: Some("sig123".to_string()),
        };

        let json = serde_json::to_string(&thinking).unwrap();
        assert!(json.contains("\"type\":\"thinking\""));
        assert!(json.contains("sig123"));
    }

    // ========================================================================
    // ToolResultContent Tests
    // ========================================================================

    #[test]
    fn test_tool_result_content_text() {
        let content = ToolResultContent::text("result");
        if let ToolResultContent::Text(t) = content {
            assert_eq!(t, "result");
        } else {
            panic!("Expected Text");
        }
    }

    #[test]
    fn test_tool_result_content_json() {
        let content = ToolResultContent::json(serde_json::json!({"key": "value"}));
        if let ToolResultContent::Json(v) = content {
            assert_eq!(v["key"], "value");
        } else {
            panic!("Expected Json");
        }
    }

    #[test]
    fn test_tool_result_content_from_string() {
        let content: ToolResultContent = "hello".into();
        if let ToolResultContent::Text(t) = content {
            assert_eq!(t, "hello");
        } else {
            panic!("Expected Text");
        }
    }

    #[test]
    fn test_tool_result_content_from_value() {
        let content: ToolResultContent = serde_json::json!({"a": 1}).into();
        if let ToolResultContent::Json(v) = content {
            assert_eq!(v["a"], 1);
        } else {
            panic!("Expected Json");
        }
    }

    // ========================================================================
    // ToolChoice Tests
    // ========================================================================

    #[test]
    fn test_tool_choice_variants() {
        let auto = ToolChoice::Auto;
        let none = ToolChoice::None;
        let required = ToolChoice::Required;
        let specific = ToolChoice::tool("fs_read");

        assert!(matches!(auto, ToolChoice::Auto));
        assert!(matches!(none, ToolChoice::None));
        assert!(matches!(required, ToolChoice::Required));
        assert!(matches!(specific, ToolChoice::Tool { name } if name == "fs_read"));
    }

    // ========================================================================
    // ModelRequest Builder Tests
    // ========================================================================

    #[test]
    fn test_model_request_builder_with_tools() {
        let tool = ToolDefinition::new(
            "test_tool",
            "A test tool",
            serde_json::json!({"type": "object"}),
        );

        let request = ModelRequest::builder("model", "system")
            .tools(vec![tool])
            .tool_choice(ToolChoice::Required)
            .build();

        assert_eq!(request.tools.len(), 1);
        assert!(matches!(request.tool_choice, ToolChoice::Required));
    }

    #[test]
    fn test_model_request_builder_defaults() {
        let request = ModelRequest::builder("model", "system").build();

        assert_eq!(request.max_tokens, 4096);
        assert!(request.temperature.is_none());
        assert!(matches!(request.tool_choice, ToolChoice::Auto));
        assert!(request.messages.is_empty());
        assert!(request.tools.is_empty());
    }

    // ========================================================================
    // ModelResponse Tests
    // ========================================================================

    #[test]
    fn test_model_response_wants_tool_use() {
        let response = ModelResponse::new(
            StopReason::ToolUse,
            Message::assistant(""),
            Usage::new(100, 50),
            ProviderTrace::new("model", 100),
        );

        assert!(response.wants_tool_use());
        assert!(!response.is_end_turn());
    }

    #[test]
    fn test_model_response_end_turn() {
        let response = ModelResponse::new(
            StopReason::EndTurn,
            Message::assistant("Done"),
            Usage::new(100, 50),
            ProviderTrace::new("model", 100),
        );

        assert!(!response.wants_tool_use());
        assert!(response.is_end_turn());
    }

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
                name: "fs_read".to_string(),
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
        assert_eq!(acc.tool_uses[0].name, "fs_read");
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
                name: "fs_ls".to_string(),
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
            name: "fs_read".to_string(),
            input_json: r#"{"path":"test.txt"}"#.to_string(),
        });
        acc.stop_reason = Some(StopReason::ToolUse);

        let response = acc.into_response(50, 100).unwrap();

        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert!(response.message.has_tool_use());

        if let ContentBlock::ToolUse { id, name, input } = &response.message.content[0] {
            assert_eq!(id, "tool1");
            assert_eq!(name, "fs_read");
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
                name: "fs_ls".to_string(),
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
                name: "fs_read".to_string(),
            },
        });
        acc.process(&StreamEvent::InputJsonDelta {
            partial_json: r#"{"path":"file.txt"}"#.to_string(),
        });
        acc.process(&StreamEvent::ContentBlockStop { index: 1 });

        assert_eq!(acc.tool_uses.len(), 2);
        assert_eq!(acc.tool_uses[0].name, "fs_ls");
        assert_eq!(acc.tool_uses[1].name, "fs_read");
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

    // ========================================================================
    // ProviderTrace Tests
    // ========================================================================

    #[test]
    fn test_provider_trace() {
        let trace = ProviderTrace::new("claude", 500).with_request_id("req123");

        assert_eq!(trace.model, "claude");
        assert_eq!(trace.latency_ms, 500);
        assert_eq!(trace.request_id, Some("req123".to_string()));
    }

    // ========================================================================
    // Usage Cache Tests
    // ========================================================================

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

    // ========================================================================
    // StreamAccumulator Input Tokens from MessageStart
    // ========================================================================

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

    // ========================================================================
    // StreamAccumulator — interleaved content / tool_use
    // ========================================================================

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
                name: "fs_read".to_string(),
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
                name: "fs_ls".to_string(),
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
        let block = ContentBlock::tool_use("id1", "cmd_run", serde_json::json!({"cmd": "ls"}));
        let json = serde_json::to_string(&block).unwrap();
        let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
        match parsed {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "id1");
                assert_eq!(name, "cmd_run");
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
        use request::ThinkingConfig;

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
}
