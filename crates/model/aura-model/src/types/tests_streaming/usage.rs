//! Provider traces, usage and token totals merged from start/delta frames.

use crate::types::*;

#[test]
fn test_provider_trace() {
    let trace = ProviderTrace::new("claude", 500).with_message_id("req123");

    assert_eq!(trace.model, "claude");
    assert_eq!(trace.latency_ms, 500);
    assert_eq!(trace.message_id.as_deref(), Some("req123"));
    assert_eq!(trace.provider_request_id, None);
    // `request_id()` falls back to `message_id` when no HTTP id is
    // populated.
    assert_eq!(trace.request_id().as_deref(), Some("req123"));
}

#[test]
fn test_provider_trace_new_split_ids() {
    // When both ids are present, `request_id()` prefers the HTTP one
    // (that's the key operators need for provider / router logs).
    let trace = ProviderTrace::new("claude", 100)
        .with_message_id("msg_01ABC")
        .with_provider_request_id("req_01XYZ");
    assert_eq!(trace.message_id.as_deref(), Some("msg_01ABC"));
    assert_eq!(trace.provider_request_id.as_deref(), Some("req_01XYZ"));
    assert_eq!(trace.request_id().as_deref(), Some("req_01XYZ"));
}

#[test]
fn test_provider_trace_accepts_legacy_request_id_alias() {
    // Old persisted bundles (`llm_calls.jsonl` entries written before
    // the split) stored the Anthropic message id under `request_id`.
    // The serde alias on `message_id` keeps those bundles loadable
    // — crucial for replay-based regression tests.
    let json = r#"{"request_id":"msg_01OLD","latency_ms":42,"model":"claude-sonnet-4"}"#;
    let trace: ProviderTrace = serde_json::from_str(json).expect("legacy shape deserializes");
    assert_eq!(trace.message_id.as_deref(), Some("msg_01OLD"));
    assert_eq!(trace.provider_request_id, None);
    assert_eq!(trace.model, "claude-sonnet-4");
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
        input_tokens: None,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });

    let response = acc.into_response(0, 100).unwrap();

    assert_eq!(response.usage.input_tokens, 200);
    assert_eq!(response.usage.cache_creation_input_tokens, Some(150));
    assert_eq!(response.usage.cache_read_input_tokens, Some(50));
    assert_eq!(response.model_used, "claude");
}

fn message_start(input: Option<u64>, creation: Option<u64>, read: Option<u64>) -> StreamEvent {
    StreamEvent::MessageStart {
        message_id: "msg".to_string(),
        model: "m".to_string(),
        input_tokens: input,
        cache_creation_input_tokens: creation,
        cache_read_input_tokens: read,
    }
}

fn message_delta(input: Option<u64>, creation: Option<u64>, read: Option<u64>) -> StreamEvent {
    StreamEvent::MessageDelta {
        stop_reason: Some(StopReason::EndTurn),
        output_tokens: 7,
        input_tokens: input,
        cache_creation_input_tokens: creation,
        cache_read_input_tokens: read,
    }
}

/// Input and cache totals as carried on a start or delta frame.
#[derive(Clone, Copy)]
struct Totals {
    input: Option<u64>,
    creation: Option<u64>,
    read: Option<u64>,
}

const fn totals(input: Option<u64>, creation: Option<u64>, read: Option<u64>) -> Totals {
    Totals {
        input,
        creation,
        read,
    }
}

struct MergeCase {
    name: &'static str,
    start: Totals,
    delta: Totals,
    expected_input: u64,
    expected_creation: Option<u64>,
    expected_read: Option<u64>,
}

const MERGE_CASES: [MergeCase; 6] = [
    MergeCase {
        name: "openrouter: zero start, delta carries totals",
        start: totals(Some(0), None, None),
        delta: totals(Some(1234), Some(10), Some(900)),
        expected_input: 1234,
        expected_creation: Some(10),
        expected_read: Some(900),
    },
    MergeCase {
        name: "openrouter: delta with null cache",
        start: totals(Some(0), None, None),
        delta: totals(Some(1234), None, None),
        expected_input: 1234,
        expected_creation: None,
        expected_read: None,
    },
    MergeCase {
        name: "anthropic: delta without totals keeps start",
        start: totals(Some(200), Some(150), Some(50)),
        delta: totals(None, None, None),
        expected_input: 200,
        expected_creation: Some(150),
        expected_read: Some(50),
    },
    MergeCase {
        name: "anthropic: cumulative delta with same totals",
        start: totals(Some(200), Some(150), Some(50)),
        delta: totals(Some(200), Some(150), Some(50)),
        expected_input: 200,
        expected_creation: Some(150),
        expected_read: Some(50),
    },
    MergeCase {
        name: "zero delta does not erase good start",
        start: totals(Some(200), Some(150), Some(50)),
        delta: totals(Some(0), Some(0), Some(0)),
        expected_input: 200,
        expected_creation: Some(150),
        expected_read: Some(50),
    },
    MergeCase {
        name: "zero delta fills unknown cache fields",
        start: totals(None, None, None),
        delta: totals(Some(0), Some(0), Some(0)),
        expected_input: 0,
        expected_creation: Some(0),
        expected_read: Some(0),
    },
];

/// Usage of a clean stream made of the case's start and delta frames.
fn merged_usage(case: &MergeCase) -> Usage {
    let mut acc = StreamAccumulator::new();
    acc.process(&message_start(
        case.start.input,
        case.start.creation,
        case.start.read,
    ));
    acc.process(&message_delta(
        case.delta.input,
        case.delta.creation,
        case.delta.read,
    ));
    acc.process(&StreamEvent::MessageStop);
    acc.into_response(0, 0).expect("clean stream").usage
}

#[test]
fn test_stream_accumulator_message_delta_usage_merge() {
    for case in &MERGE_CASES {
        let name = case.name;
        let usage = merged_usage(case);
        assert_eq!(usage.input_tokens, case.expected_input, "{name}: input");
        assert_eq!(usage.output_tokens, 7, "{name}: output");
        assert_eq!(
            usage.cache_creation_input_tokens, case.expected_creation,
            "{name}: cache_creation"
        );
        assert_eq!(
            usage.cache_read_input_tokens, case.expected_read,
            "{name}: cache_read"
        );
    }
}

#[test]
fn test_stream_accumulator_message_start_without_cache_keeps_known_cache() {
    // A later start frame (e.g. the OpenAI Responses decoder's closing
    // start) without cache numbers must not erase earlier ones.
    let mut acc = StreamAccumulator::new();
    acc.process(&message_start(Some(10), Some(4), Some(6)));
    acc.process(&message_start(Some(12), None, None));
    assert_eq!(acc.input_tokens, 12);
    assert_eq!(acc.cache_creation_input_tokens, Some(4));
    assert_eq!(acc.cache_read_input_tokens, Some(6));
}

#[test]
fn test_stream_accumulator_finalize_uses_fallback_input_tokens() {
    let acc = StreamAccumulator::new();
    let response = acc.into_response(999, 50).unwrap();
    assert_eq!(response.usage.input_tokens, 999);
    assert_eq!(response.stop_reason, StopReason::EndTurn);
    assert!(response.message.content.is_empty());
}
