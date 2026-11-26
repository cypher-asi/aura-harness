
use super::*;
use crate::error::ReasonerError;
use crate::types::{Message, ProviderTrace, Role, StopReason, Usage};

fn mk_response() -> ModelResponse {
    ModelResponse::new(
        StopReason::ToolUse,
        Message::new(
            Role::Assistant,
            vec![
                ContentBlock::Text {
                    text: "hello".into(),
                },
                ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({ "path": "a" }),
                },
            ],
        ),
        Usage::new(1, 2),
        ProviderTrace::new("test-model", 10),
    )
}

#[tokio::test]
async fn response_stream_from_response_emits_items_then_completed() {
    let response = mk_response();
    let mut stream = response_stream_from_response(response);
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.expect("test stream never yields Err"));
    }
    assert_eq!(events.len(), 3);
    assert!(matches!(
        &events[0],
        ResponseEvent::OutputItemDone(OutputItem::Message { text }) if text == "hello"
    ));
    assert!(matches!(
        &events[1],
        ResponseEvent::OutputItemDone(OutputItem::ToolUse { id, name, .. })
            if id == "toolu_1" && name == "read_file"
    ));
    assert!(matches!(
        &events[2],
        ResponseEvent::Completed {
            end_turn: Some(false),
            ..
        }
    ));
}

#[tokio::test]
async fn response_stream_from_event_stream_aggregates_blocks() {
    use crate::types::StreamContentType;
    let events: Vec<Result<StreamEvent, ReasonerError>> = vec![
        Ok(StreamEvent::MessageStart {
            message_id: "msg".into(),
            model: "test".into(),
            input_tokens: Some(3),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::Text,
        }),
        Ok(StreamEvent::TextDelta {
            text: "hello ".into(),
        }),
        Ok(StreamEvent::TextDelta {
            text: "world".into(),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::ContentBlockStart {
            index: 1,
            content_type: StreamContentType::ToolUse {
                id: "toolu_1".into(),
                name: "read_file".into(),
            },
        }),
        Ok(StreamEvent::InputJsonDelta {
            partial_json: "{\"path\":\"a\"}".into(),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 1 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::ToolUse),
            output_tokens: 5,
            input_tokens: None,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
        Ok(StreamEvent::MessageStop),
    ];
    let underlying: StreamEventStream = Box::pin(futures_util::stream::iter(events));
    let mut stream = response_stream_from_event_stream(underlying);
    let mut collected = Vec::new();
    while let Some(event) = stream.next().await {
        collected.push(event.expect("test stream never yields Err"));
    }
    // Keepalives and incremental text/thinking deltas now
    // interleave with completed items (one per delta / start /
    // message_delta frame); filter them out to assert the
    // completed-item backbone is unchanged.
    let items: Vec<&ResponseEvent> = collected
        .iter()
        .filter(|e| {
            !matches!(
                e,
                ResponseEvent::Keepalive(_)
                    | ResponseEvent::OutputTextDelta(_)
                    | ResponseEvent::OutputThinkingDelta(_)
                    | ResponseEvent::OutputToolUseStart { .. }
                    | ResponseEvent::OutputToolInputDelta(_)
            )
        })
        .collect();
    assert_eq!(items.len(), 3);
    assert!(matches!(
        items[0],
        ResponseEvent::OutputItemDone(OutputItem::Message { text }) if text == "hello world"
    ));
    assert!(matches!(
        items[1],
        ResponseEvent::OutputItemDone(OutputItem::ToolUse { name, .. }) if name == "read_file"
    ));
    assert!(matches!(
        items[2],
        ResponseEvent::Completed {
            end_turn: Some(false),
            ..
        }
    ));
    // The text deltas must surface as incremental, content-bearing
    // events (which also reset the consumer's per-event timeout so
    // a slow-but-alive stream is not mistaken for a dead one).
    assert!(
        collected.iter().any(|e| matches!(
            e,
            ResponseEvent::OutputTextDelta(text) if text == "hello " || text == "world"
        )),
        "text deltas must surface as OutputTextDelta"
    );
}

#[tokio::test]
async fn response_stream_completed_usage_takes_message_delta_totals() {
    // OpenRouter's Anthropic-compatible endpoint, non-Anthropic
    // upstream: `message_start` reports 0 input tokens and the real
    // totals arrive on `message_delta`.
    let events: Vec<Result<StreamEvent, ReasonerError>> = vec![
        Ok(StreamEvent::MessageStart {
            message_id: "gen-1".into(),
            model: "openai/gpt-5".into(),
            input_tokens: Some(0),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::ToolUse),
            output_tokens: 56,
            input_tokens: Some(1234),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: Some(900),
        }),
        Ok(StreamEvent::MessageStop),
    ];
    let underlying: StreamEventStream = Box::pin(futures_util::stream::iter(events));
    let mut stream = response_stream_from_event_stream(underlying);
    let mut completed_usage = None;
    while let Some(event) = stream.next().await {
        if let Ok(ResponseEvent::Completed { usage, .. }) = event {
            completed_usage = Some(usage);
        }
    }
    let usage = completed_usage.expect("stream emits Completed");
    assert_eq!(usage.input_tokens, 1234);
    assert_eq!(usage.output_tokens, 56);
    assert_eq!(usage.cache_creation_input_tokens, None);
    assert_eq!(usage.cache_read_input_tokens, Some(900));
}

#[tokio::test]
async fn response_stream_from_event_stream_surfaces_phase_tagged_keepalives() {
    use crate::types::StreamContentType;
    let events: Vec<Result<StreamEvent, ReasonerError>> = vec![
        Ok(StreamEvent::MessageStart {
            message_id: "msg".into(),
            model: "test".into(),
            input_tokens: Some(1),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
        Ok(StreamEvent::Ping),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::Thinking,
        }),
        Ok(StreamEvent::ThinkingDelta {
            thinking: "let me think".into(),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageStop),
    ];
    let underlying: StreamEventStream = Box::pin(futures_util::stream::iter(events));
    let mut stream = response_stream_from_event_stream(underlying);
    let mut phases = Vec::new();
    let mut thinking_text = String::new();
    while let Some(event) = stream.next().await {
        match &event {
            Ok(ResponseEvent::Keepalive(phase)) => phases.push(*phase),
            Ok(ResponseEvent::OutputThinkingDelta(chunk)) => thinking_text.push_str(chunk),
            _ => {}
        }
    }
    // message_start -> Connecting, ping -> Ping,
    // content_block_start(thinking) -> Thinking keepalive.
    assert!(phases.contains(&StreamPhase::Connecting));
    assert!(phases.contains(&StreamPhase::Ping));
    assert!(phases.contains(&StreamPhase::Thinking));
    // The thinking delta itself now carries its content through.
    assert_eq!(thinking_text, "let me think");
}

#[tokio::test]
async fn response_stream_from_event_stream_surfaces_tool_start_and_input() {
    use crate::types::StreamContentType;
    let events: Vec<Result<StreamEvent, ReasonerError>> = vec![
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::ToolUse {
                id: "toolu_1".into(),
                name: "write_file".into(),
            },
        }),
        Ok(StreamEvent::InputJsonDelta {
            partial_json: "{\"path\":".into(),
        }),
        Ok(StreamEvent::InputJsonDelta {
            partial_json: "\"a.html\"}".into(),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageStop),
    ];
    let underlying: StreamEventStream = Box::pin(futures_util::stream::iter(events));
    let mut stream = response_stream_from_event_stream(underlying);
    let mut collected = Vec::new();
    while let Some(event) = stream.next().await {
        collected.push(event.expect("test stream never yields Err"));
    }
    assert!(matches!(
        &collected[0],
        ResponseEvent::OutputToolUseStart { id, name } if id == "toolu_1" && name == "write_file"
    ));
    let input: String = collected
        .iter()
        .filter_map(|e| match e {
            ResponseEvent::OutputToolInputDelta(chunk) => Some(chunk.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(input, "{\"path\":\"a.html\"}");
    assert!(collected.iter().any(|e| matches!(
            e,
            ResponseEvent::OutputItemDone(OutputItem::ToolUse { input, .. }) if input["path"] == "a.html"
        )));
}

#[tokio::test]
async fn response_stream_from_event_stream_propagates_transport_err() {
    use crate::types::StreamContentType;
    let events: Vec<Result<StreamEvent, ReasonerError>> = vec![
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::Text,
        }),
        Err(ReasonerError::Internal("boom".into())),
    ];
    let underlying: StreamEventStream = Box::pin(futures_util::stream::iter(events));
    let mut stream = response_stream_from_event_stream(underlying);
    let mut got_err = false;
    while let Some(event) = stream.next().await {
        if let Err(StreamError::TransportClosed { .. }) = event {
            got_err = true;
            break;
        }
    }
    assert!(got_err, "transport error must surface as TransportClosed");
}

#[tokio::test]
async fn response_stream_from_event_stream_preserves_partial_tool_abort() {
    use crate::types::StreamContentType;
    let events: Vec<Result<StreamEvent, ReasonerError>> = vec![
        Ok(StreamEvent::MessageStart {
            message_id: "msg_partial".into(),
            model: "test".into(),
            input_tokens: Some(1),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::ToolUse {
                id: "toolu_partial".into(),
                name: "write_file".into(),
            },
        }),
        Ok(StreamEvent::InputJsonDelta {
            partial_json: "{\"path\":\"src/".into(),
        }),
        Ok(StreamEvent::Error {
            message: "overloaded_error: upstream flaked".into(),
            request_id: None,
        }),
    ];
    let underlying: StreamEventStream = Box::pin(futures_util::stream::iter(events));
    let mut stream = response_stream_from_event_stream(underlying);

    // Skip the message_start keepalive and the tool start / input
    // deltas that precede the terminal abort.
    let terminal = loop {
        let event = stream.next().await.expect("stream should emit abort");
        if !matches!(
            event,
            Ok(ResponseEvent::Keepalive(_)
                | ResponseEvent::OutputToolUseStart { .. }
                | ResponseEvent::OutputToolInputDelta(_))
        ) {
            break event;
        }
    };
    match terminal {
        Err(StreamError::StreamAbortedWithPartial {
            reason,
            partial_tool_use: Some(partial),
        }) => {
            assert!(reason.contains("overloaded_error"));
            assert_eq!(partial.tool_use_id, "toolu_partial");
            assert_eq!(partial.tool_name, "write_file");
            assert_eq!(partial.partial_json, "{\"path\":\"src/");
        }
        other => panic!("expected partial abort, got {other:?}"),
    }
}
