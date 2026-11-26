use super::*;
use crate::env::ENV_TEST_LOCK;

#[test]
fn defaults_are_const_evaluable() {
    const _DEFAULTS: AuraConfig = AuraConfig::defaults();
}

#[test]
fn from_env_returns_a_config() {
    // Hold the crate-wide env lock so submodule tests that
    // intentionally set malformed env values for their own
    // parse-error assertions don't race this happy-path probe.
    let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = AuraConfig::from_env().expect("from_env defaults must succeed");
}

#[test]
fn a_bad_gate_value_leaves_the_other_settings_in_place() {
    let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(env::AURA_GATE_TARGET, "high");
    std::env::set_var(env::AURA_GATE_EXPLORE, "0.2");
    std::env::set_var(env::AURA_SUBAGENT_MAX_DEPTH, "3");
    let cfg = AuraConfig::from_env();
    std::env::remove_var(env::AURA_GATE_TARGET);
    std::env::remove_var(env::AURA_GATE_EXPLORE);
    std::env::remove_var(env::AURA_SUBAGENT_MAX_DEPTH);
    let cfg = cfg.expect("a bad Gate value is not an error");
    assert_eq!(cfg.gate.target, GateConfig::defaults().target);
    assert_eq!(cfg.gate.explore_rate, 0.2);
    assert_eq!(cfg.subagent.max_depth, 3);
}

#[test]
fn install_for_test_restores_previous_on_drop() {
    let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let original = current();
    {
        let mut overridden = AuraConfig::defaults();
        overridden.agent.compaction.disabled = !original.agent.compaction.disabled;
        let _guard = install_for_test(overridden.clone());
        assert_eq!(
            current().agent.compaction.disabled,
            overridden.agent.compaction.disabled,
        );
    }
    assert_eq!(
        current().agent.compaction.disabled,
        original.agent.compaction.disabled,
    );
}

#[test]
fn loaded_returns_an_owned_snapshot() {
    let _: AuraConfig = loaded();
    let _: AgentConfig = agent();
    let _: ReasonerConfig = reasoner();
}

#[test]
fn install_for_test_is_observable_through_accessors() {
    let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let baseline = current();
    let mut overridden = baseline.clone();
    overridden.agent.compaction.disabled = !baseline.agent.compaction.disabled;
    let _guard = install_for_test(overridden.clone());
    assert_eq!(
        agent().compaction.disabled,
        overridden.agent.compaction.disabled,
        "agent() must observe install_for_test swaps"
    );
}
