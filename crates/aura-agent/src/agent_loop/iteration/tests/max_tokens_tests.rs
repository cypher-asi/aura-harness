use aura_reasoner::{ContentBlock, Message, ModelResponse, ProviderTrace, Role, Usage};

use crate::agent_loop::iteration::handle_max_tokens;
use crate::agent_loop::{AgentLoopConfig, LoopState};

fn tool_use_response(tool_name: &str, path: Option<&str>) -> ModelResponse {
    let input = match path {
        Some(p) => serde_json::json!({"path": p, "content": "stub"}),
        None => serde_json::json!({"content": "stub"}),
    };
    let message = Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "toolu_1".into(),
            name: tool_name.into(),
            input,
        }],
    };
    ModelResponse {
        stop_reason: aura_reasoner::StopReason::MaxTokens,
        message,
        usage: Usage::default(),
        trace: ProviderTrace::default(),
        model_used: String::new(),
    }
}

fn find_tool_result_text(state: &LoopState) -> Vec<String> {
    let Some(last) = state.messages.last() else {
        return Vec::new();
    };
    last.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult { content, .. } => match content {
                aura_reasoner::ToolResultContent::Text(t) => Some(t.clone()),
                aura_reasoner::ToolResultContent::Json(v) => Some(v.to_string()),
            },
            _ => None,
        })
        .collect()
}

/// Build a realistic in-progress conversation: a prior user turn
/// followed by the assistant message with the truncated tool_use
/// block. handle_max_tokens will push a tool_result Message after
/// the assistant message, which sanitize::validate_and_repair then
/// keeps paired correctly.
fn seed_state_with(config: &AgentLoopConfig, response: &ModelResponse) -> LoopState {
    let initial = vec![Message::user("go write the file"), response.message.clone()];
    LoopState::new(config, initial)
}

#[test]
fn handle_max_tokens_for_write_file_carries_path_hint() {
    let config = AgentLoopConfig::default();
    let response = tool_use_response("write_file", Some("crates/foo/src/lib.rs"));
    let mut state = seed_state_with(&config, &response);

    assert!(handle_max_tokens(&config, &response, &mut state));
    let texts = find_tool_result_text(&state);
    assert_eq!(texts.len(), 1, "one tool_result per pending tool_use");
    let text = &texts[0];
    assert!(
        text.contains("crates/foo/src/lib.rs"),
        "path should appear in the recovery hint, got: {text}"
    );
    assert!(
        text.contains("edit_file") && text.contains("append_after_eof"),
        "recovery pattern should name edit_file + append_after_eof, got: {text}"
    );
}

#[test]
fn handle_max_tokens_for_non_write_tool_uses_generic_text() {
    let config = AgentLoopConfig::default();
    let response = tool_use_response("read_file", Some("src/main.rs"));
    let mut state = seed_state_with(&config, &response);

    assert!(handle_max_tokens(&config, &response, &mut state));
    let texts = find_tool_result_text(&state);
    assert_eq!(texts.len(), 1);
    assert!(
        !texts[0].contains("append_after_eof"),
        "non-write tools should not get the append_after_eof hint"
    );
    assert!(texts[0].contains("truncated"));
}

#[test]
fn handle_max_tokens_for_edit_file_suggests_splitting_the_edit() {
    // Regression: previously `edit_file` fell through to the
    // generic branch ("try a simpler approach"), which gave the
    // model no concrete recovery path. The harness logs showed
    // repeated `edit_file` truncations as a result. The hint
    // must name `edit_file` explicitly and steer toward splitting.
    let config = AgentLoopConfig::default();
    let response = tool_use_response("edit_file", Some("crates/foo/src/lib.rs"));
    let mut state = seed_state_with(&config, &response);

    assert!(handle_max_tokens(&config, &response, &mut state));
    let texts = find_tool_result_text(&state);
    assert_eq!(texts.len(), 1);
    let text = &texts[0];
    assert!(
        text.contains("crates/foo/src/lib.rs"),
        "path must appear in edit_file recovery hint: {text}"
    );
    assert!(
        text.to_ascii_lowercase().contains("split")
            || text.to_ascii_lowercase().contains("smaller"),
        "edit_file hint should steer toward splitting the edit: {text}"
    );
}

#[test]
fn handle_max_tokens_sets_budget_restore_flag() {
    // The flag is the contract between `handle_max_tokens` and
    // `LoopState::begin_iteration`: truncation implies "next turn
    // needs full budget". Without this, a tapered budget carries
    // into the retry and the model hits `max_tokens` again.
    let config = AgentLoopConfig::default();
    let response = tool_use_response("edit_file", Some("src/x.rs"));
    let mut state = seed_state_with(&config, &response);
    assert!(!state.thinking.restore_next_iteration, "precondition");

    assert!(handle_max_tokens(&config, &response, &mut state));
    assert!(
        state.thinking.restore_next_iteration,
        "handle_max_tokens must arm the budget-restore flag"
    );
}

#[test]
fn begin_iteration_restores_budget_and_clears_flag() {
    // Given a tapered budget and the restore flag set,
    // begin_iteration must lift the budget back to `max_tokens`
    // and clear the flag so the *next* iteration can taper again.
    let config = AgentLoopConfig::default();
    let mut state = LoopState::new(&config, vec![Message::user("go")]);
    state.thinking.budget = 512;
    state.thinking.restore_next_iteration = true;

    // Iteration number is irrelevant for the restore path — the
    // flag short-circuits before the taper branch.
    state.begin_iteration(&config, 99);

    assert_eq!(
        state.thinking.budget, config.max_tokens,
        "budget must be restored to max_tokens after truncation"
    );
    assert!(
        !state.thinking.restore_next_iteration,
        "flag must be cleared after a single restore"
    );
}

#[test]
fn begin_iteration_respects_min_budget_floor() {
    // Even after a long run with aggressive tapering, the budget
    // must never fall below `thinking_min_budget`. The floor is
    // what keeps a multi-KB tool-call JSON serializable.
    let config = AgentLoopConfig {
        thinking_taper_after: 0,
        thinking_taper_factor: 0.1,
        ..AgentLoopConfig::default()
    };
    let mut state = LoopState::new(&config, vec![Message::user("go")]);

    for i in 0..50 {
        state.begin_iteration(&config, i);
        assert!(
            state.thinking.budget >= config.thinking_min_budget,
            "budget dropped below floor at iteration {i}: {}",
            state.thinking.budget
        );
    }
}
