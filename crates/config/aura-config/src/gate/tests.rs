use super::env::{MIN_TRIES_ACCEPTS, RATE_ACCEPTS, REVIEW_ACCEPTS, SWITCH_ACCEPTS};
use super::*;
use crate::env::{
    AURA_GATE_CHECK, AURA_GATE_CRITERIA, AURA_GATE_DELEGATE, AURA_GATE_EXPLORE, AURA_GATE_JUDGE,
    AURA_GATE_MIN_TRIES, AURA_GATE_NUDGE, AURA_GATE_REVIEW, AURA_GATE_REVIEW_SAMPLE,
    AURA_GATE_TARGET, ENV_TEST_LOCK,
};
use crate::ConfigError;

fn clear_gate_env() {
    std::env::remove_var(AURA_GATE_TARGET);
    std::env::remove_var(AURA_GATE_MIN_TRIES);
    std::env::remove_var(AURA_GATE_EXPLORE);
    std::env::remove_var(AURA_GATE_DELEGATE);
    std::env::remove_var(AURA_GATE_JUDGE);
    std::env::remove_var(AURA_GATE_NUDGE);
    std::env::remove_var(AURA_GATE_CHECK);
    std::env::remove_var(AURA_GATE_REVIEW);
    std::env::remove_var(AURA_GATE_CRITERIA);
    std::env::remove_var(AURA_GATE_REVIEW_SAMPLE);
}

#[test]
fn defaults_are_stable() {
    let cfg = GateConfig::defaults();
    assert_eq!(cfg.target, 0.85);
    assert_eq!(cfg.min_tries, 10);
    assert_eq!(cfg.explore_rate, 0.05);
    assert!(cfg.delegate);
    assert!(cfg.judge);
    assert!(cfg.nudge);
    assert!(cfg.harness_check);
    assert_eq!(cfg.review, ReviewMode::Log);
    assert_eq!(cfg.criteria, ReviewMode::Log);
    assert_eq!(cfg.review_sample, 0.25);
}

#[test]
fn without_exploring_changes_only_the_explore_rate() {
    let cfg = GateConfig {
        delegate: false,
        review: ReviewMode::On,
        ..GateConfig::defaults()
    };
    assert_eq!(
        cfg.without_exploring(),
        GateConfig {
            explore_rate: 0.0,
            ..cfg
        }
    );
}

#[test]
fn the_criteria_are_on_logged_or_off() {
    let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (raw, criteria) in [
        ("on", ReviewMode::On),
        ("Log", ReviewMode::Log),
        ("off", ReviewMode::Off),
        ("false", ReviewMode::Off),
    ] {
        clear_gate_env();
        std::env::set_var(AURA_GATE_CRITERIA, raw);
        assert_eq!(GateConfig::from_env().criteria, criteria, "{raw}");
    }
    clear_gate_env();
}

#[test]
fn the_review_is_on_logged_or_off() {
    let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (raw, review) in [
        ("on", ReviewMode::On),
        ("LOG", ReviewMode::Log),
        ("off", ReviewMode::Off),
        ("0", ReviewMode::Off),
        ("yes", ReviewMode::On),
    ] {
        clear_gate_env();
        std::env::set_var(AURA_GATE_REVIEW, raw);
        assert_eq!(GateConfig::from_env().review, review, "{raw}");
    }
    clear_gate_env();
    assert_eq!(ReviewMode::On.to_string(), "on");
}

#[test]
fn unset_env_keeps_the_defaults() {
    let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_gate_env();
    let (cfg, rejected) = GateConfig::read_env();
    assert_eq!(cfg, GateConfig::defaults());
    assert!(rejected.is_empty(), "{rejected:?}");
}

#[test]
fn env_overrides_apply() {
    let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_gate_env();
    std::env::set_var(AURA_GATE_TARGET, "0.9");
    std::env::set_var(AURA_GATE_MIN_TRIES, " 4 ");
    std::env::set_var(AURA_GATE_EXPLORE, "0");
    std::env::set_var(AURA_GATE_DELEGATE, "off");
    std::env::set_var(AURA_GATE_JUDGE, "no");
    std::env::set_var(AURA_GATE_NUDGE, "off");
    std::env::set_var(AURA_GATE_CHECK, "0");
    std::env::set_var(AURA_GATE_REVIEW, "off");
    std::env::set_var(AURA_GATE_CRITERIA, "on");
    std::env::set_var(AURA_GATE_REVIEW_SAMPLE, "1");
    let cfg = GateConfig::from_env();
    clear_gate_env();
    assert_eq!(
        cfg,
        GateConfig {
            target: 0.9,
            min_tries: 4,
            explore_rate: 0.0,
            delegate: false,
            judge: false,
            nudge: false,
            harness_check: false,
            review: ReviewMode::Off,
            criteria: ReviewMode::On,
            review_sample: 1.0,
        }
    );
}

#[test]
fn delegation_stays_on_unless_turned_off() {
    let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (raw, delegate) in [
        ("off", false),
        ("0", false),
        ("No", false),
        ("on", true),
        ("TRUE", true),
        ("maybe", true),
    ] {
        clear_gate_env();
        std::env::set_var(AURA_GATE_DELEGATE, raw);
        assert_eq!(GateConfig::from_env().delegate, delegate, "{raw}");
    }
    clear_gate_env();
}

#[test]
fn a_bad_value_is_reported_with_its_env_var_and_accepted_values() {
    let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (name, raw, accepts, default) in [
        (AURA_GATE_TARGET, "85%", RATE_ACCEPTS, "0.85"),
        (AURA_GATE_TARGET, "1.5", RATE_ACCEPTS, "0.85"),
        (AURA_GATE_TARGET, "NaN", RATE_ACCEPTS, "0.85"),
        (AURA_GATE_EXPLORE, "-0.1", RATE_ACCEPTS, "0.05"),
        (AURA_GATE_MIN_TRIES, "0", MIN_TRIES_ACCEPTS, "10"),
        (AURA_GATE_MIN_TRIES, "ten", MIN_TRIES_ACCEPTS, "10"),
        (AURA_GATE_MIN_TRIES, "2.5", MIN_TRIES_ACCEPTS, "10"),
        (AURA_GATE_DELEGATE, "maybe", SWITCH_ACCEPTS, "on"),
        (AURA_GATE_JUDGE, "sometimes", SWITCH_ACCEPTS, "on"),
        (AURA_GATE_NUDGE, "sometimes", SWITCH_ACCEPTS, "on"),
        (AURA_GATE_CHECK, "sometimes", SWITCH_ACCEPTS, "on"),
        (AURA_GATE_REVIEW, "sometimes", REVIEW_ACCEPTS, "log"),
        (AURA_GATE_CRITERIA, "sometimes", REVIEW_ACCEPTS, "log"),
        (AURA_GATE_REVIEW_SAMPLE, "25%", RATE_ACCEPTS, "0.25"),
        (AURA_GATE_REVIEW_SAMPLE, "2", RATE_ACCEPTS, "0.25"),
    ] {
        clear_gate_env();
        std::env::set_var(name, raw);
        let (cfg, rejected) = GateConfig::read_env();
        assert_eq!(cfg, GateConfig::defaults(), "{name}={raw}");
        assert_eq!(rejected.len(), 1, "{name}={raw}: {rejected:?}");
        let err = rejected[0].to_string();
        assert!(err.contains(name), "{name}={raw}: {err}");
        assert!(err.contains(&format!("`{raw}`")), "{name}={raw}: {err}");
        let says = format!("expected {accepts}; using the default {default}");
        assert!(err.contains(&says), "{name}={raw}: {err}");
    }
    clear_gate_env();
}

#[test]
fn a_bad_value_keeps_only_its_own_default() {
    let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_gate_env();
    std::env::set_var(AURA_GATE_TARGET, "0.9");
    std::env::set_var(AURA_GATE_MIN_TRIES, "0");
    std::env::set_var(AURA_GATE_EXPLORE, "lots");
    std::env::set_var(AURA_GATE_DELEGATE, "off");
    let (cfg, rejected) = GateConfig::read_env();
    clear_gate_env();
    assert_eq!(
        cfg,
        GateConfig {
            target: 0.9,
            min_tries: DEFAULT_MIN_TRIES,
            explore_rate: DEFAULT_EXPLORE_RATE,
            delegate: false,
            ..GateConfig::defaults()
        }
    );
    let names: Vec<&str> = rejected
        .iter()
        .map(|ConfigError::InvalidValue { name, .. }| *name)
        .collect();
    assert_eq!(names, [AURA_GATE_MIN_TRIES, AURA_GATE_EXPLORE]);
}
