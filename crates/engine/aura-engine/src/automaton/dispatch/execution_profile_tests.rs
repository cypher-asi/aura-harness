
use super::*;
#[test]
fn task_profile_preserves_effort_and_limits() {
    let mut config = AgentRunnerConfig::for_agent("test-model");
    let profile = aura_core_protocol::ModelSelection {
        max_turns: Some(7),
        reasoning_effort: Some(aura_core_protocol::ReasoningEffort::XHigh),
        ..Default::default()
    };
    apply_execution_profile(&mut config, Some(&profile)).unwrap();
    assert_eq!(config.max_agentic_iterations, 7);
    assert_eq!(
        config.reasoning_effort,
        Some(aura_model::ThinkingEffort::XHigh)
    );
}
#[test]
fn zero_execution_limits_are_rejected() {
    let profile = aura_core_protocol::ModelSelection {
        max_turns: Some(0),
        ..Default::default()
    };
    assert!(
        apply_execution_profile(&mut AgentRunnerConfig::for_agent("test"), Some(&profile)).is_err()
    );
}
