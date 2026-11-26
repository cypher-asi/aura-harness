use super::*;

fn next_request(state: &mut LoopState, config: &AgentLoopConfig) -> ModelRequest {
    state.begin_iteration(config, 1);
    state.build_request(config, &[], 1).unwrap()
}

#[test]
fn replies_get_the_models_whole_output_limit_and_reason_less_after_a_cut_off() {
    let config = AgentLoopConfig::for_agent("claude/claude-opus-5-5");
    let mut state = LoopState::new(&config, vec![Message::user("write the module")]);
    let first = next_request(&mut state, &config);
    assert_eq!(first.max_tokens.get(), 128_000);
    assert_eq!(first.thinking_effort, Some(ThinkingEffort::High));

    state.reason_less();
    let second = next_request(&mut state, &config);
    assert_eq!(second.max_tokens.get(), 128_000);
    assert_eq!(second.thinking_effort, Some(ThinkingEffort::Medium));

    state.reason_less();
    let later = next_request(&mut state, &config);
    assert_eq!(later.thinking_effort, Some(ThinkingEffort::Low));
    assert_eq!(
        next_request(&mut state, &config).thinking_effort,
        Some(ThinkingEffort::Low)
    );
}

#[test]
fn a_routed_effort_is_lowered_after_a_cut_off_too() {
    let config = AgentLoopConfig {
        routed_thinking_effort: Some(ThinkingEffort::Max),
        ..AgentLoopConfig::for_agent("claude/claude-opus-5-5")
    };
    let mut state = LoopState::new(&config, vec![Message::user("write the module")]);
    assert_eq!(
        next_request(&mut state, &config).thinking_effort,
        Some(ThinkingEffort::Max)
    );
    state.reason_less();
    assert_eq!(
        next_request(&mut state, &config).thinking_effort,
        Some(ThinkingEffort::High)
    );
}

#[test]
fn a_chosen_effort_stands() {
    let config = AgentLoopConfig {
        user_thinking_effort: Some(ThinkingEffort::Max),
        ..AgentLoopConfig::for_agent("claude/claude-opus-5-5")
    };
    let mut state = LoopState::new(&config, vec![Message::user("write the module")]);

    state.reason_less();
    let request = next_request(&mut state, &config);
    assert_eq!(request.thinking_effort, Some(ThinkingEffort::Max));
}
