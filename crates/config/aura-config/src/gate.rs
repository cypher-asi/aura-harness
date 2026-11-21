//! How Gate decides a model has proven itself, how often it tries a
//! cheaper one, whether the lead agent must delegate its work, whether a
//! judge model reviews routed turns that failed, whether work a check
//! verifies starts where it is expected to cost least, and what Gate
//! does about a routed turn that ran no check.
//!
//! ## Owned env vars
//!
//! | Var | Type | Default | Field |
//! | --- | --- | --- | --- |
//! | `AURA_GATE_TARGET` | f64 (0..=1) | `0.85` | [`GateConfig::target`] |
//! | `AURA_GATE_MIN_TRIES` | u32 (>= 1) | `10` | [`GateConfig::min_tries`] |
//! | `AURA_GATE_EXPLORE` | f64 (0..=1) | `0.05` | [`GateConfig::explore_rate`] |
//! | `AURA_GATE_DELEGATE` | `on` / `off` | `on` | [`GateConfig::delegate`] |
//! | `AURA_GATE_JUDGE` | `on` / `off` | `on` | [`GateConfig::judge`] |
//! | `AURA_GATE_CASCADE` | `on` / `off` | `on` | [`GateConfig::cascade`] |
//! | `AURA_GATE_NUDGE` | `on` / `off` | `on` | [`GateConfig::nudge`] |
//! | `AURA_GATE_CHECK` | `on` / `off` | `on` | [`GateConfig::harness_check`] |
//! | `AURA_GATE_SUSPECT` | `on` / `off` | `on` | [`GateConfig::suspect_judge`] |
//! | `AURA_GATE_REVIEW` | `on` / `log` / `off` | `log` | [`GateConfig::review`] |
//!
//! A value outside what a variable accepts keeps that setting's default
//! and logs a warning naming the variable and its accepted values; the
//! other settings, Gate's and the rest of `aura-config`'s, still apply.

use std::fmt::Display;

use serde::{Deserialize, Serialize};

use crate::env::{
    lookup_string, AURA_GATE_CASCADE, AURA_GATE_CHECK, AURA_GATE_DELEGATE, AURA_GATE_EXPLORE,
    AURA_GATE_JUDGE, AURA_GATE_MIN_TRIES, AURA_GATE_NUDGE, AURA_GATE_REVIEW, AURA_GATE_SUSPECT,
    AURA_GATE_TARGET, FALSY_LITERALS, TRUTHY_LITERALS,
};
use crate::ConfigError;

const DEFAULT_TARGET: f64 = 0.85;
const DEFAULT_MIN_TRIES: u32 = 10;
const DEFAULT_EXPLORE_RATE: f64 = 0.05;

const RATE_ACCEPTS: &str = "a number from 0 to 1";
const MIN_TRIES_ACCEPTS: &str = "a whole number of at least 1";
const SWITCH_ACCEPTS: &str = "on or off (also 1/0, true/false, yes/no)";
const REVIEW_ACCEPTS: &str = "on, log or off (also 1/0, true/false, yes/no)";

/// What Gate's judge does about work a routed turn is about to let
/// stand.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewMode {
    /// It is not asked.
    Off,
    /// It reviews the work and its verdict is logged, but the turn goes
    /// on as if it had not been asked.
    #[default]
    Log,
    /// It reviews the work, and work it finds unfinished goes back to
    /// the model once, then on to a stronger one.
    On,
}

impl Display for ReviewMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Off => "off",
            Self::Log => "log",
            Self::On => "on",
        })
    }
}

/// Gate's routing settings.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "snake_case")]
pub struct GateConfig {
    /// Success rate a model needs on a category to become its pick.
    ///
    /// env: `AURA_GATE_TARGET` (default: `0.85`)
    pub target: f64,
    /// Counted outcomes a model needs on a category before its success
    /// rate is trusted.
    ///
    /// env: `AURA_GATE_MIN_TRIES` (default: `10`)
    pub min_tries: u32,
    /// Share of routed work sent to the next cheaper model instead of
    /// the pick, to learn how it does.
    ///
    /// env: `AURA_GATE_EXPLORE` (default: `0.05`)
    pub explore_rate: f64,
    /// Whether the lead agent under Gate must delegate: no file edits and
    /// only check and inspection commands of its own. When off it keeps
    /// every tool and is only told to delegate.
    ///
    /// env: `AURA_GATE_DELEGATE` (default: `on`)
    pub delegate: bool,
    /// Whether a judge model reviews each turn Gate routed that failed
    /// (an error, or its last test or build failing) and says whether the
    /// model was at fault, which decides whether the failure counts
    /// against it and the turn moves to a stronger model. When off, every
    /// such failure counts and moves the turn up.
    ///
    /// env: `AURA_GATE_JUDGE` (default: `on`)
    pub judge: bool,
    /// Whether work a check verifies (code changes and debugging) starts
    /// on the model it is expected to cost least from, counting the
    /// stronger models a failed attempt moves up to. When off it is
    /// routed like other work, by success rate.
    ///
    /// env: `AURA_GATE_CASCADE` (default: `on`)
    pub cascade: bool,
    /// Whether a routed turn below the strongest tier that changed files
    /// but ran no test or build is asked once, on the same model, to run
    /// the tests that cover its change.
    ///
    /// env: `AURA_GATE_NUDGE` (default: `on`)
    pub nudge: bool,
    /// Whether the harness runs the project's tests (`AURA_DOD_TEST_COMMAND`,
    /// else the command its manifest implies) on a routed turn below the
    /// strongest tier that changed files and still ran none. A failure
    /// moves the turn up as a failed check of its own does.
    ///
    /// env: `AURA_GATE_CHECK` (default: `on`)
    pub harness_check: bool,
    /// Whether the judge reviews a routed turn below the strongest tier
    /// that ran no check but looks wrong: code work that changed no file,
    /// a reply that gives up, or a turn stopped on a limit. Needs
    /// [`Self::judge`] on as well.
    ///
    /// env: `AURA_GATE_SUSPECT` (default: `on`)
    pub suspect_judge: bool,
    /// Whether the judge reviews work a routed turn below the strongest
    /// tier is about to let stand, when the work was classified hard or
    /// shows a sign of failure the attempt log backs: it sees the request,
    /// everything the turn changed, its checks and its reply. With `on`,
    /// work it finds unfinished goes back to the same model once with
    /// what it found, then on to a stronger model. With `log` its verdict
    /// is only written to the attempt log. Needs [`Self::judge`] on as
    /// well.
    ///
    /// env: `AURA_GATE_REVIEW` (default: `log`)
    pub review: ReviewMode,
}

impl GateConfig {
    /// Compile-time defaults. No env access.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            target: DEFAULT_TARGET,
            min_tries: DEFAULT_MIN_TRIES,
            explore_rate: DEFAULT_EXPLORE_RATE,
            delegate: true,
            judge: true,
            cascade: true,
            nudge: true,
            harness_check: true,
            suspect_judge: true,
            review: ReviewMode::Log,
        }
    }

    /// Apply env overrides. A variable set to a value it does not accept
    /// keeps its default, with a warning naming it and what it accepts.
    #[must_use]
    pub fn from_env() -> Self {
        let (cfg, rejected) = Self::read_env();
        for err in &rejected {
            tracing::warn!(error = %err, "aura-config: ignoring an invalid Gate setting");
        }
        cfg
    }

    /// The settings from the environment, and the variables whose value
    /// was not accepted.
    fn read_env() -> (Self, Vec<ConfigError>) {
        let mut cfg = Self::defaults();
        let mut rejected = Vec::new();
        if let Some(v) = read(
            AURA_GATE_TARGET,
            RATE_ACCEPTS,
            DEFAULT_TARGET,
            parse_rate,
            &mut rejected,
        ) {
            cfg.target = v;
        }
        if let Some(v) = read(
            AURA_GATE_MIN_TRIES,
            MIN_TRIES_ACCEPTS,
            DEFAULT_MIN_TRIES,
            parse_min_tries,
            &mut rejected,
        ) {
            cfg.min_tries = v;
        }
        if let Some(v) = read(
            AURA_GATE_EXPLORE,
            RATE_ACCEPTS,
            DEFAULT_EXPLORE_RATE,
            parse_rate,
            &mut rejected,
        ) {
            cfg.explore_rate = v;
        }
        if let Some(v) = read(
            AURA_GATE_DELEGATE,
            SWITCH_ACCEPTS,
            "on",
            parse_switch,
            &mut rejected,
        ) {
            cfg.delegate = v;
        }
        if let Some(v) = read(
            AURA_GATE_JUDGE,
            SWITCH_ACCEPTS,
            "on",
            parse_switch,
            &mut rejected,
        ) {
            cfg.judge = v;
        }
        if let Some(v) = read(
            AURA_GATE_CASCADE,
            SWITCH_ACCEPTS,
            "on",
            parse_switch,
            &mut rejected,
        ) {
            cfg.cascade = v;
        }
        for (name, setting) in [
            (AURA_GATE_NUDGE, &mut cfg.nudge),
            (AURA_GATE_CHECK, &mut cfg.harness_check),
            (AURA_GATE_SUSPECT, &mut cfg.suspect_judge),
        ] {
            if let Some(v) = read(name, SWITCH_ACCEPTS, "on", parse_switch, &mut rejected) {
                *setting = v;
            }
        }
        if let Some(v) = read(
            AURA_GATE_REVIEW,
            REVIEW_ACCEPTS,
            Self::defaults().review,
            parse_review,
            &mut rejected,
        ) {
            cfg.review = v;
        }
        (cfg, rejected)
    }
}

impl Default for GateConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

/// The value of `name` when it is set and `parse` accepts it. A value it
/// rejects is added to `rejected`, saying what `name` accepts and that
/// `default` stays.
fn read<T>(
    name: &'static str,
    accepts: &str,
    default: impl Display,
    parse: fn(&str) -> Option<T>,
    rejected: &mut Vec<ConfigError>,
) -> Option<T> {
    let raw = lookup_string(name)?;
    let value = parse(&raw);
    if value.is_none() {
        rejected.push(ConfigError::InvalidValue {
            name,
            raw,
            message: format!("expected {accepts}; using the default {default}"),
        });
    }
    value
}

fn parse_rate(raw: &str) -> Option<f64> {
    raw.parse::<f64>().ok().filter(|v| (0.0..=1.0).contains(v))
}

fn parse_min_tries(raw: &str) -> Option<u32> {
    raw.parse::<u32>().ok().filter(|v| *v >= 1)
}

fn parse_review(raw: &str) -> Option<ReviewMode> {
    if raw.eq_ignore_ascii_case("log") {
        return Some(ReviewMode::Log);
    }
    parse_switch(raw).map(|on| if on { ReviewMode::On } else { ReviewMode::Off })
}

fn parse_switch(raw: &str) -> Option<bool> {
    let matches = |literals: &[&str]| literals.iter().any(|l| l.eq_ignore_ascii_case(raw));
    if matches(TRUTHY_LITERALS) {
        Some(true)
    } else if matches(FALSY_LITERALS) {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::ENV_TEST_LOCK;

    fn clear_gate_env() {
        std::env::remove_var(AURA_GATE_TARGET);
        std::env::remove_var(AURA_GATE_MIN_TRIES);
        std::env::remove_var(AURA_GATE_EXPLORE);
        std::env::remove_var(AURA_GATE_DELEGATE);
        std::env::remove_var(AURA_GATE_JUDGE);
        std::env::remove_var(AURA_GATE_CASCADE);
        std::env::remove_var(AURA_GATE_NUDGE);
        std::env::remove_var(AURA_GATE_CHECK);
        std::env::remove_var(AURA_GATE_SUSPECT);
        std::env::remove_var(AURA_GATE_REVIEW);
    }

    #[test]
    fn defaults_are_stable() {
        let cfg = GateConfig::defaults();
        assert_eq!(cfg.target, 0.85);
        assert_eq!(cfg.min_tries, 10);
        assert_eq!(cfg.explore_rate, 0.05);
        assert!(cfg.delegate);
        assert!(cfg.judge);
        assert!(cfg.cascade);
        assert!(cfg.nudge);
        assert!(cfg.harness_check);
        assert!(cfg.suspect_judge);
        assert_eq!(cfg.review, ReviewMode::Log);
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
        std::env::set_var(AURA_GATE_CASCADE, "off");
        std::env::set_var(AURA_GATE_NUDGE, "off");
        std::env::set_var(AURA_GATE_CHECK, "0");
        std::env::set_var(AURA_GATE_SUSPECT, "false");
        std::env::set_var(AURA_GATE_REVIEW, "off");
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
                cascade: false,
                nudge: false,
                harness_check: false,
                suspect_judge: false,
                review: ReviewMode::Off,
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
        for (name, raw, accepts) in [
            (AURA_GATE_TARGET, "85%", RATE_ACCEPTS),
            (AURA_GATE_TARGET, "1.5", RATE_ACCEPTS),
            (AURA_GATE_TARGET, "NaN", RATE_ACCEPTS),
            (AURA_GATE_EXPLORE, "-0.1", RATE_ACCEPTS),
            (AURA_GATE_MIN_TRIES, "0", MIN_TRIES_ACCEPTS),
            (AURA_GATE_MIN_TRIES, "ten", MIN_TRIES_ACCEPTS),
            (AURA_GATE_MIN_TRIES, "2.5", MIN_TRIES_ACCEPTS),
            (AURA_GATE_DELEGATE, "maybe", SWITCH_ACCEPTS),
            (AURA_GATE_JUDGE, "sometimes", SWITCH_ACCEPTS),
            (AURA_GATE_CASCADE, "sometimes", SWITCH_ACCEPTS),
            (AURA_GATE_NUDGE, "sometimes", SWITCH_ACCEPTS),
            (AURA_GATE_CHECK, "sometimes", SWITCH_ACCEPTS),
            (AURA_GATE_SUSPECT, "sometimes", SWITCH_ACCEPTS),
            (AURA_GATE_REVIEW, "sometimes", REVIEW_ACCEPTS),
        ] {
            clear_gate_env();
            std::env::set_var(name, raw);
            let (cfg, rejected) = GateConfig::read_env();
            assert_eq!(cfg, GateConfig::defaults(), "{name}={raw}");
            assert_eq!(rejected.len(), 1, "{name}={raw}: {rejected:?}");
            let err = rejected[0].to_string();
            assert!(err.contains(name), "{name}={raw}: {err}");
            assert!(err.contains(&format!("`{raw}`")), "{name}={raw}: {err}");
            assert!(err.contains(accepts), "{name}={raw}: {err}");
            assert!(err.contains("using the default"), "{name}={raw}: {err}");
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
}
