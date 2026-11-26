use super::*;

#[test]
fn configure_loop_config_simple_caps_iterations() {
    let config = AgentRunnerConfig::for_agent("claude-test-model");
    let loop_cfg = configure_loop_config(TaskComplexity::Simple, &config, 3, "system".into());
    assert!(loop_cfg.max_iterations <= 15);
}

#[test]
fn configure_loop_config_complex_uses_full_budget() {
    let config = AgentRunnerConfig::for_agent("claude-test-model");
    let loop_cfg = configure_loop_config(TaskComplexity::Complex, &config, 3, "system".into());
    assert_eq!(loop_cfg.max_iterations, config.max_agentic_iterations);
}

#[test]
fn configure_loop_config_maps_all_fields() {
    let config = AgentRunnerConfig::for_agent("claude-test-model");
    let loop_cfg = configure_loop_config(TaskComplexity::Standard, &config, 3, "system".into());
    assert_eq!(loop_cfg.billing_reason, "aura_task");
    assert_eq!(loop_cfg.auto_build_cooldown, 1);
}

#[test]
fn finalize_loop_result_uses_text_when_present() {
    let result = AgentLoopResult {
        total_text: "Did the thing".to_string(),
        total_input_tokens: 100,
        total_output_tokens: 50,
        ..AgentLoopResult::default()
    };
    let exec = finalize_loop_result(result);
    assert_eq!(exec.notes, "Did the thing");
    assert_eq!(exec.input_tokens, 100);
    assert_eq!(exec.output_tokens, 50);
}

#[test]
fn finalize_loop_result_default_notes_when_empty() {
    let result = AgentLoopResult::default();
    let exec = finalize_loop_result(result);
    assert!(exec.notes.contains("agentic tool-use loop"));
}
