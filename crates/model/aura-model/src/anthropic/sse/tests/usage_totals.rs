use super::*;

#[test]
fn message_delta_forwards_input_and_cache_totals() {
    let event = parse_sse_event(
        r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":1234,"output_tokens":56,"cache_creation_input_tokens":null,"cache_read_input_tokens":900,"output_tokens_details":{"reasoning_tokens":10},"server_tool_use":{"web_search_requests":0},"cost":0.0012}}"#,
    );
    match event {
        Some(StreamEvent::MessageDelta {
            stop_reason,
            output_tokens,
            input_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
        }) => {
            assert_eq!(stop_reason, Some(StopReason::ToolUse));
            assert_eq!(output_tokens, 56);
            assert_eq!(input_tokens, Some(1234));
            assert_eq!(cache_creation_input_tokens, None);
            assert_eq!(cache_read_input_tokens, Some(900));
        }
        other => panic!("Expected MessageDelta, got {other:?}"),
    }
}

#[test]
fn openrouter_zero_start_takes_totals_from_message_delta() {
    let usage = usage_from_sse_blocks(&[
        OPENROUTER_START_ZERO,
        r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":1234,"output_tokens":56,"cache_creation_input_tokens":200,"cache_read_input_tokens":900}}"#,
        MESSAGE_STOP,
    ]);
    assert_eq!(usage.input_tokens, 1234);
    assert_eq!(usage.output_tokens, 56);
    assert_eq!(usage.cache_creation_input_tokens, Some(200));
    assert_eq!(usage.cache_read_input_tokens, Some(900));
}

#[test]
fn openrouter_delta_with_null_cache_fields_keeps_them_unknown() {
    let usage = usage_from_sse_blocks(&[
        OPENROUTER_START_ZERO,
        r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":1234,"output_tokens":56,"cache_creation_input_tokens":null,"cache_read_input_tokens":null}}"#,
        MESSAGE_STOP,
    ]);
    assert_eq!(usage.input_tokens, 1234);
    assert_eq!(usage.output_tokens, 56);
    assert_eq!(usage.cache_creation_input_tokens, None);
    assert_eq!(usage.cache_read_input_tokens, None);
}

#[test]
fn message_start_with_null_input_tokens_parses() {
    for usage_json in [
        r#"{"input_tokens":null,"output_tokens":0}"#,
        r#"{"output_tokens":0}"#,
        r#"{"input_tokens":null,"cache_creation_input_tokens":null,"cache_read_input_tokens":null}"#,
    ] {
        let block = format!(
            "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"gen-1\",\"model\":\"deepseek/deepseek-v4\",\"usage\":{usage_json}}}}}"
        );
        let event = parse_sse_event(&block);
        assert!(
            matches!(
                event,
                Some(StreamEvent::MessageStart {
                    input_tokens: None,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    ..
                })
            ),
            "usage {usage_json} -> {event:?}"
        );
    }
}

#[test]
fn anthropic_delta_with_only_output_tokens_keeps_start_totals() {
    let usage = usage_from_sse_blocks(&[
        ANTHROPIC_START,
        r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":56}}"#,
        MESSAGE_STOP,
    ]);
    assert_eq!(usage.input_tokens, 1234);
    assert_eq!(usage.output_tokens, 56);
    assert_eq!(usage.cache_creation_input_tokens, Some(100));
    assert_eq!(usage.cache_read_input_tokens, Some(900));
}

#[test]
fn anthropic_cumulative_delta_with_same_totals_is_stable() {
    let usage = usage_from_sse_blocks(&[
        ANTHROPIC_START,
        r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":1234,"output_tokens":56,"cache_creation_input_tokens":100,"cache_read_input_tokens":900}}"#,
        MESSAGE_STOP,
    ]);
    assert_eq!(usage.input_tokens, 1234);
    assert_eq!(usage.output_tokens, 56);
    assert_eq!(usage.cache_creation_input_tokens, Some(100));
    assert_eq!(usage.cache_read_input_tokens, Some(900));
}

#[test]
fn delta_zero_or_null_totals_do_not_erase_start_totals() {
    let usage = usage_from_sse_blocks(&[
        ANTHROPIC_START,
        r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":0,"output_tokens":56,"cache_creation_input_tokens":0,"cache_read_input_tokens":null}}"#,
        MESSAGE_STOP,
    ]);
    assert_eq!(usage.input_tokens, 1234);
    assert_eq!(usage.cache_creation_input_tokens, Some(100));
    assert_eq!(usage.cache_read_input_tokens, Some(900));
}
