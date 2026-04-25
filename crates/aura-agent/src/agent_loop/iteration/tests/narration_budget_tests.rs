use aura_reasoner::{ContentBlock, Message, ModelResponse, ProviderTrace, Role, Usage};

use crate::agent_loop::iteration::update_narration_budget;
use crate::agent_loop::{AgentLoopConfig, LoopState};
use crate::constants::{NARRATION_TOKEN_HARD_BUDGET, NARRATION_TOKEN_SOFT_BUDGET};

fn text_only_response(output_tokens: u64) -> ModelResponse {
    let message = Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "planning and narrating without a tool call".into(),
        }],
    };
    ModelResponse {
        stop_reason: aura_reasoner::StopReason::EndTurn,
        message,
        usage: Usage {
            output_tokens,
            ..Usage::default()
        },
        trace: ProviderTrace::default(),
        model_used: String::new(),
    }
}

fn tool_use_response(output_tokens: u64) -> ModelResponse {
    let message = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Text {
                text: "brief preamble".into(),
            },
            ContentBlock::ToolUse {
                id: "toolu_narr".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "src/lib.rs"}),
            },
        ],
    };
    ModelResponse {
        stop_reason: aura_reasoner::StopReason::ToolUse,
        message,
        usage: Usage {
            output_tokens,
            ..Usage::default()
        },
        trace: ProviderTrace::default(),
        model_used: String::new(),
    }
}

fn fresh_state() -> LoopState {
    let config = AgentLoopConfig::default();
    LoopState::new(&config, vec![Message::user("do the task")])
}

fn last_user_text(state: &LoopState) -> Option<String> {
    state.messages.iter().rev().find_map(|m| {
        if matches!(m.role, Role::User) {
            m.content.iter().find_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
        } else {
            None
        }
    })
}

#[test]
fn narration_counter_resets_on_tool_use() {
    let mut state = fresh_state();
    state.counters.consecutive_narration_tokens = 900;
    state.counters.last_turn_had_tool_call = false;

    let response = tool_use_response(300);
    assert!(!update_narration_budget(None, &mut state, &response));
    assert_eq!(state.counters.consecutive_narration_tokens, 0);
    assert!(state.counters.last_turn_had_tool_call);
    assert!(state.result.stop_reason_override.is_none());
}

#[test]
fn narration_counter_accumulates_across_toolfree_turns() {
    let mut state = fresh_state();

    let first = text_only_response(400);
    assert!(!update_narration_budget(None, &mut state, &first));
    assert_eq!(state.counters.consecutive_narration_tokens, 400);
    assert!(!state.counters.last_turn_had_tool_call);

    let second = text_only_response(500);
    assert!(!update_narration_budget(None, &mut state, &second));
    assert_eq!(state.counters.consecutive_narration_tokens, 900);
    assert!(!state.counters.last_turn_had_tool_call);
    assert!(state.result.stop_reason_override.is_none());
}

#[test]
fn soft_budget_injects_steering_message() {
    let mut state = fresh_state();
    let messages_before = state.messages.len();

    let big = text_only_response(NARRATION_TOKEN_SOFT_BUDGET as u64);
    assert!(!update_narration_budget(None, &mut state, &big));

    assert_eq!(
        state.counters.consecutive_narration_tokens, 0,
        "soft budget should reset the counter after injection"
    );
    assert_eq!(
        state.messages.len(),
        messages_before + 1,
        "exactly one steering user message should be appended"
    );

    let injected = last_user_text(&state).expect("steering user text");
    assert!(
        injected.contains("harness steering"),
        "should carry the [harness steering] prefix, got: {injected}"
    );
    assert!(
        injected.contains("call exactly ONE tool"),
        "should tell the model to call exactly one tool, got: {injected}"
    );
    assert!(
        injected.contains("12000 bytes"),
        "should cite the Phase 1 write chunk cap, got: {injected}"
    );
    assert!(
        state.result.stop_reason_override.is_none(),
        "soft budget alone should not set a stop reason"
    );
}

#[test]
fn hard_budget_terminates_or_signals() {
    let mut state = fresh_state();

    let exhaust = text_only_response(NARRATION_TOKEN_HARD_BUDGET as u64);
    let should_break = update_narration_budget(None, &mut state, &exhaust);

    assert!(
        should_break,
        "hard budget must signal the loop to break immediately"
    );
    assert_eq!(
        state.result.stop_reason_override.as_deref(),
        Some("narration_budget_exhausted"),
        "stop_reason_override must carry the narration_budget_exhausted code"
    );
    assert!(
        state.result.stalled,
        "hard budget should mark the result as stalled for downstream observability"
    );
}
