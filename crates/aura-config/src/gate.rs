//! How Gate decides a model has proven itself, and how often it tries a
//! cheaper one.
//!
//! ## Owned env vars
//!
//! | Var | Type | Default | Field |
//! | --- | --- | --- | --- |
//! | `AURA_GATE_TARGET` | f64 (0..=1) | `0.85` | [`GateConfig::target`] |
//! | `AURA_GATE_MIN_TRIES` | u32 (>= 1) | `10` | [`GateConfig::min_tries`] |
//! | `AURA_GATE_EXPLORE` | f64 (0..=1) | `0.05` | [`GateConfig::explore_rate`] |

use serde::{Deserialize, Serialize};

use crate::env::{lookup_numeric, AURA_GATE_EXPLORE, AURA_GATE_MIN_TRIES, AURA_GATE_TARGET};
use crate::ConfigError;

const DEFAULT_TARGET: f64 = 0.85;
const DEFAULT_MIN_TRIES: u32 = 10;
const DEFAULT_EXPLORE_RATE: f64 = 0.05;

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
}

impl GateConfig {
    /// Compile-time defaults. No env access.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            target: DEFAULT_TARGET,
            min_tries: DEFAULT_MIN_TRIES,
            explore_rate: DEFAULT_EXPLORE_RATE,
        }
    }

    /// Apply env overrides.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when an override is not a number, a rate
    /// is outside `0..=1`, or `AURA_GATE_MIN_TRIES` is 0.
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut cfg = Self::defaults();
        if let Some(v) = lookup_rate(AURA_GATE_TARGET)? {
            cfg.target = v;
        }
        if let Some(v) = lookup_numeric::<u32>(AURA_GATE_MIN_TRIES)? {
            if v == 0 {
                return Err(invalid(
                    AURA_GATE_MIN_TRIES,
                    v.to_string(),
                    "must be at least 1",
                ));
            }
            cfg.min_tries = v;
        }
        if let Some(v) = lookup_rate(AURA_GATE_EXPLORE)? {
            cfg.explore_rate = v;
        }
        Ok(cfg)
    }
}

impl Default for GateConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

/// A rate from `name`: a number from 0 to 1.
fn lookup_rate(name: &'static str) -> Result<Option<f64>, ConfigError> {
    match lookup_numeric::<f64>(name)? {
        Some(v) if !(0.0..=1.0).contains(&v) => {
            Err(invalid(name, v.to_string(), "must be between 0 and 1"))
        }
        other => Ok(other),
    }
}

fn invalid(name: &'static str, raw: String, message: &str) -> ConfigError {
    ConfigError::InvalidValue {
        name,
        raw,
        message: message.to_string(),
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
    }

    #[test]
    fn defaults_are_stable() {
        let cfg = GateConfig::defaults();
        assert_eq!(cfg.target, 0.85);
        assert_eq!(cfg.min_tries, 10);
        assert_eq!(cfg.explore_rate, 0.05);
    }

    #[test]
    fn unset_env_keeps_the_defaults() {
        let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_gate_env();
        assert_eq!(GateConfig::from_env().unwrap(), GateConfig::defaults());
    }

    #[test]
    fn env_overrides_apply() {
        let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_gate_env();
        std::env::set_var(AURA_GATE_TARGET, "0.9");
        std::env::set_var(AURA_GATE_MIN_TRIES, " 4 ");
        std::env::set_var(AURA_GATE_EXPLORE, "0");
        let cfg = GateConfig::from_env().unwrap();
        clear_gate_env();
        assert_eq!(
            cfg,
            GateConfig {
                target: 0.9,
                min_tries: 4,
                explore_rate: 0.0,
            }
        );
    }

    #[test]
    fn bad_values_name_their_env_var() {
        let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for (name, raw) in [
            (AURA_GATE_TARGET, "85%"),
            (AURA_GATE_TARGET, "1.5"),
            (AURA_GATE_EXPLORE, "-0.1"),
            (AURA_GATE_MIN_TRIES, "0"),
            (AURA_GATE_MIN_TRIES, "ten"),
        ] {
            clear_gate_env();
            std::env::set_var(name, raw);
            let err = GateConfig::from_env().unwrap_err().to_string();
            assert!(err.contains(name), "{name}={raw}: {err}");
        }
        clear_gate_env();
    }
}
