use super::*;

// -----------------------------------------------------------------
// E.4 mandatory pump tests
// -----------------------------------------------------------------

/// `pump_emits_per_delta_events` (E.4 mandatory): with an
/// `event_tx` plumbed in, the pump emits at minimum a `TextDelta`
/// for a finished `OutputItem::Message`, a `ThinkingDelta` +
/// `ThinkingComplete` for `OutputItem::Thinking`, and a
/// `ToolStart` + `ToolInputSnapshot` pair for an
/// `OutputItem::ToolUse`. This was the gate that let the
/// `use_stream_pump` default flip without regressing the
/// chat-stream UX (see audit note); Phase 7 retired the toggle so
/// this test now pins the production contract directly.
#[tokio::test(start_paused = true)]
async fn pump_emits_per_delta_events() {
    let executor = CountingExecutor::default();
    let config = AgentLoopConfig::for_agent("claude-test-model");
    let events = vec![
        ResponseEvent::OutputItemDone(OutputItem::Thinking {
            thinking: "thought".into(),
            signature: None,
        }),
        ResponseEvent::OutputItemDone(OutputItem::Message {
            text: "hello".into(),
        }),
        mk_call("toolu_a", "read_file"),
        completed(true),
    ];
    let events = drive_collecting_events(&config, &executor, mk_stream(events)).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentLoopEvent::TextDelta(t) if t == "hello")),
        "pump must emit TextDelta for Message blocks: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentLoopEvent::ThinkingDelta(t) if t == "thought")),
        "pump must emit ThinkingDelta for Thinking blocks: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentLoopEvent::ThinkingComplete)),
        "pump must emit ThinkingComplete after a Thinking block: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentLoopEvent::ToolStart { id, name } if id == "toolu_a" && name == "read_file"
        )),
        "pump must emit ToolStart for OutputItemDone(ToolUse): {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentLoopEvent::ToolInputSnapshot { id, name, .. }
                if id == "toolu_a" && name == "read_file"
        )),
        "pump must emit ToolInputSnapshot for OutputItemDone(ToolUse): {events:?}"
    );
}

/// A tool call shows up when its block opens, and its arguments follow
/// as throttled partial snapshots while they stream, so a long
/// `write_file` body is not a silent gap.
#[tokio::test(start_paused = true)]
async fn pump_streams_tool_input_while_the_block_is_open() {
    let executor = CountingExecutor::default();
    let config = AgentLoopConfig::for_agent("claude-test-model");
    let events = vec![
        ResponseEvent::OutputToolUseStart {
            id: "toolu_w".into(),
            name: "write_file".into(),
        },
        ResponseEvent::OutputToolInputDelta("{\"path\":\"a.html\",".into()),
        ResponseEvent::OutputToolInputDelta("\"content\":\"<h1>".into()),
        ResponseEvent::OutputToolInputDelta("</h1>\"}".into()),
        ResponseEvent::OutputItemDone(OutputItem::ToolUse {
            id: "toolu_w".into(),
            name: "write_file".into(),
            input: serde_json::json!({ "path": "a.html", "content": "<h1></h1>" }),
        }),
        completed(false),
    ];
    let stream: ResponseEventStream = Box::pin(futures_util::StreamExt::then(
        futures_util::stream::iter(events),
        |event| async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok::<_, aura_model::StreamError>(event)
        },
    ));
    let events = drive_collecting_events(&config, &executor, stream).await;
    let starts = events
        .iter()
        .filter(|e| matches!(e, AgentLoopEvent::ToolStart { id, .. } if id == "toolu_w"))
        .count();
    assert_eq!(
        starts, 1,
        "ToolStart must not repeat at block close: {events:?}"
    );
    let snapshots: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentLoopEvent::ToolInputSnapshot { input, .. } => Some(input.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        snapshots.first(),
        Some(&"{\"path\":\"a.html\","),
        "partial input must stream before the block closes: {events:?}"
    );
    let full: serde_json::Value =
        serde_json::from_str(snapshots.last().expect("final snapshot")).expect("full JSON");
    assert_eq!(full["content"], "<h1></h1>");
}

/// A reply streamed as three thinking deltas, then the text `Done.`.
fn thinking_then_text() -> Vec<Result<StreamEvent, aura_model::ReasonerError>> {
    vec![
        Ok(StreamEvent::MessageStart {
            message_id: "msg".into(),
            model: "test".into(),
            input_tokens: Some(1),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::Thinking,
        }),
        Ok(StreamEvent::ThinkingDelta {
            thinking: "Let me ".into(),
        }),
        Ok(StreamEvent::ThinkingDelta {
            thinking: "check the ".into(),
        }),
        Ok(StreamEvent::ThinkingDelta {
            thinking: "files.".into(),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::ContentBlockStart {
            index: 1,
            content_type: StreamContentType::Text,
        }),
        Ok(StreamEvent::TextDelta {
            text: "Done.".into(),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 1 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            output_tokens: 5,
            input_tokens: None,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

/// The deltas `pick` takes out of `events`, in order.
fn deltas<'a>(
    events: &'a [AgentLoopEvent],
    pick: impl Fn(&'a AgentLoopEvent) -> Option<&'a str>,
) -> Vec<&'a str> {
    events.iter().filter_map(pick).collect()
}

/// Regression: thinking (and text) must stream to the client
/// *incrementally* — one `ThinkingDelta` per wire delta as it
/// arrives — not a single coarse delta at block close. Feeds a
/// multi-delta thinking block through the real
/// `response_stream_from_event_stream` adapter (the production lift
/// path) and asserts each chunk surfaces on its own, the per-chunk
/// order is preserved, the block is NOT re-emitted whole at close,
/// and a single `ThinkingComplete` marks the end.
#[tokio::test(start_paused = true)]
async fn pump_streams_thinking_incrementally_not_just_at_block_close() {
    let executor = CountingExecutor::default();
    let config = AgentLoopConfig::for_agent("claude-test-model");

    let wire = thinking_then_text();
    let underlying: StreamEventStream = Box::pin(futures_util::stream::iter(wire));
    let stream = response_stream_from_event_stream(underlying);

    let events = drive_collecting_events(&config, &executor, stream).await;

    // Each thinking chunk must surface on its own, in order — proof
    // that the client sees the reasoning stream live instead of one
    // block-close dump.
    let thinking = deltas(&events, |e| match e {
        AgentLoopEvent::ThinkingDelta(t) => Some(t.as_str()),
        _ => None,
    });
    assert_eq!(
        thinking,
        vec!["Let me ", "check the ", "files."],
        "thinking must stream one delta per wire frame, in order: {events:?}"
    );

    // Exactly one ThinkingComplete after the block.
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentLoopEvent::ThinkingComplete))
            .count(),
        1,
        "exactly one ThinkingComplete should mark end-of-thinking: {events:?}"
    );

    // Text streams the same way; the incremental delta is NOT
    // re-emitted whole at block close (dedup), so "Done." appears once.
    let text = deltas(&events, |e| match e {
        AgentLoopEvent::TextDelta(t) => Some(t.as_str()),
        _ => None,
    });
    assert_eq!(
        text,
        vec!["Done."],
        "assistant text must stream once, not be duplicated at block close: {events:?}"
    );
}
