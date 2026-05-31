//! Phase 6c regression test — confirm
//! [`aura_runtime::memory_observer::turn_summary_from_result`] copies every
//! documented `TurnSummary` field from a non-default
//! [`aura_agent::AgentLoopResult`].
//!
//! The intent is to fail loudly if a future change adds a `TurnSummary`
//! field without also growing the adapter (or vice-versa) — the
//! inversion's whole point is that the memory crate consumes
//! `TurnSummary` and the runtime owns the copy.

use aura_agent::AgentLoopResult;
use aura_model_reasoner::Message;
use aura_runtime::memory_observer::turn_summary_from_result;

#[test]
fn turn_summary_copies_every_documented_field() {
    let messages = vec![Message::user("hello"), Message::assistant("world")];
    let result = AgentLoopResult {
        timed_out: true,
        stalled: true,
        llm_error: Some("rate_limited".to_string()),
        total_text: "assistant output text".to_string(),
        total_input_tokens: 1_234,
        total_output_tokens: 567,
        iterations: 7,
        messages: messages.clone(),
        // Everything below is on AgentLoopResult but intentionally NOT
        // mirrored on TurnSummary — the memory pipeline doesn't read
        // it. Leaving them at defaults keeps the test focused on the
        // mirrored subset and documents the boundary.
        ..AgentLoopResult::default()
    };

    let summary = turn_summary_from_result(&result);

    assert!(summary.timed_out, "timed_out must round-trip");
    assert!(summary.stalled, "stalled must round-trip");
    assert_eq!(
        summary.llm_error.as_deref(),
        Some("rate_limited"),
        "llm_error must round-trip",
    );
    assert_eq!(summary.total_text, "assistant output text");
    assert_eq!(summary.total_input_tokens, 1_234);
    assert_eq!(summary.total_output_tokens, 567);
    assert_eq!(summary.iterations, 7);
    assert_eq!(
        summary.messages.len(),
        messages.len(),
        "messages must round-trip (cloned)",
    );
    assert_eq!(summary.messages[0].text_content(), "hello");
    assert_eq!(summary.messages[1].text_content(), "world");
}

#[test]
fn turn_summary_default_when_result_default() {
    let result = AgentLoopResult::default();
    let summary = turn_summary_from_result(&result);

    assert!(!summary.timed_out);
    assert!(!summary.stalled);
    assert!(summary.llm_error.is_none());
    assert!(summary.total_text.is_empty());
    assert_eq!(summary.total_input_tokens, 0);
    assert_eq!(summary.total_output_tokens, 0);
    assert_eq!(summary.iterations, 0);
    assert!(summary.messages.is_empty());
}
