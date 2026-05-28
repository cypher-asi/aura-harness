//! Subagent derivation + spawn budget knobs (Phase 4a).
//!
//! These values are consumed by the Phase 6a derivation path
//! (`aura-agent-steering`) and the future fleet daemon. They live in
//! `aura-config` ahead of those consumers so the per-subagent
//! budget vocabulary stays the same across phases.
//!
//! ## Invariants ([rules.md §13])
//!
//! - `max_depth` strictly bounds derivation chain length; the
//!   derivation path raises `DerivationError::DepthExceeded` when
//!   the next-depth would meet or exceed this value (see plan §4).
//! - Every field is default-stable; zero-config users see today's
//!   behaviour (no subagent derivation actually fires before
//!   Phase 6a).
//! - [`SubagentConfig::defaults`] is `const fn` so the Phase 6a
//!   derivation default can read the same numbers without paying a
//!   per-call clone.
//!
//! ## Owned env vars
//!
//! | Var | Type | Default | Field |
//! | --- | --- | --- | --- |
//! | `AURA_SUBAGENT_MAX_DEPTH` | u32 | `8` | [`SubagentConfig::max_depth`] |
//! | `AURA_SUBAGENT_DEFAULT_MAX_TOKENS` | u32 | `64_000` | [`SubagentConfig::default_max_tokens`] |
//! | `AURA_SUBAGENT_DEFAULT_MAX_ITERATIONS` | u32 | `50` | [`SubagentConfig::default_max_iterations`] |
//! | `AURA_SUBAGENT_DEFAULT_TIMEOUT_MS` | u64 | `300_000` | [`SubagentConfig::default_timeout_ms`] |

use serde::{Deserialize, Serialize};

use crate::env::{
    lookup_numeric, AURA_SUBAGENT_DEFAULT_MAX_ITERATIONS, AURA_SUBAGENT_DEFAULT_MAX_TOKENS,
    AURA_SUBAGENT_DEFAULT_TIMEOUT_MS, AURA_SUBAGENT_MAX_DEPTH,
};

const DEFAULT_MAX_DEPTH: u32 = 8;
const DEFAULT_MAX_TOKENS: u32 = 64_000;
const DEFAULT_MAX_ITERATIONS: u32 = 50;
const DEFAULT_TIMEOUT_MS: u64 = 300_000;

/// Subagent derivation + spawn budget knobs. See the module-level
/// docs for invariants.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "snake_case")]
pub struct SubagentConfig {
    /// Maximum subagent depth (root = 0; first-gen child = 1; ...).
    ///
    /// `DerivationError::DepthExceeded` is raised when next-depth
    /// would meet or exceed this value.
    pub max_depth: u32,
    /// Default per-subagent token budget.
    pub default_max_tokens: u32,
    /// Default per-subagent iteration budget.
    pub default_max_iterations: u32,
    /// Default per-subagent wall-clock budget (ms).
    pub default_timeout_ms: u64,
}

impl SubagentConfig {
    /// Compile-time defaults. No env access.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            default_max_tokens: DEFAULT_MAX_TOKENS,
            default_max_iterations: DEFAULT_MAX_ITERATIONS,
            default_timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }

    /// Apply env overrides.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ConfigError`] when one of the numeric
    /// overrides is non-empty but unparseable.
    pub fn from_env() -> Result<Self, crate::ConfigError> {
        let mut cfg = Self::defaults();
        if let Some(v) = lookup_numeric::<u32>(AURA_SUBAGENT_MAX_DEPTH)? {
            cfg.max_depth = v;
        }
        if let Some(v) = lookup_numeric::<u32>(AURA_SUBAGENT_DEFAULT_MAX_TOKENS)? {
            cfg.default_max_tokens = v;
        }
        if let Some(v) = lookup_numeric::<u32>(AURA_SUBAGENT_DEFAULT_MAX_ITERATIONS)? {
            cfg.default_max_iterations = v;
        }
        if let Some(v) = lookup_numeric::<u64>(AURA_SUBAGENT_DEFAULT_TIMEOUT_MS)? {
            cfg.default_timeout_ms = v;
        }
        Ok(cfg)
    }
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::ENV_TEST_LOCK;

    fn clear_subagent_env() {
        std::env::remove_var(AURA_SUBAGENT_MAX_DEPTH);
        std::env::remove_var(AURA_SUBAGENT_DEFAULT_MAX_TOKENS);
        std::env::remove_var(AURA_SUBAGENT_DEFAULT_MAX_ITERATIONS);
        std::env::remove_var(AURA_SUBAGENT_DEFAULT_TIMEOUT_MS);
    }

    #[test]
    fn defaults_are_stable() {
        let cfg = SubagentConfig::defaults();
        assert_eq!(cfg.max_depth, DEFAULT_MAX_DEPTH);
        assert_eq!(cfg.default_max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(cfg.default_max_iterations, DEFAULT_MAX_ITERATIONS);
        assert_eq!(cfg.default_timeout_ms, DEFAULT_TIMEOUT_MS);
    }

    #[test]
    fn defaults_are_const_evaluable() {
        const _DEFAULTS: SubagentConfig = SubagentConfig::defaults();
    }

    #[test]
    fn from_env_falls_back_to_defaults_when_unset() {
        let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_subagent_env();
        let cfg = SubagentConfig::from_env().expect("defaults must parse");
        assert_eq!(cfg, SubagentConfig::defaults());
        clear_subagent_env();
    }

    #[test]
    fn from_env_applies_overrides() {
        let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_subagent_env();
        std::env::set_var(AURA_SUBAGENT_MAX_DEPTH, "3");
        std::env::set_var(AURA_SUBAGENT_DEFAULT_MAX_TOKENS, "8000");
        std::env::set_var(AURA_SUBAGENT_DEFAULT_MAX_ITERATIONS, "12");
        std::env::set_var(AURA_SUBAGENT_DEFAULT_TIMEOUT_MS, "5000");
        let cfg = SubagentConfig::from_env().expect("overrides must parse");
        assert_eq!(cfg.max_depth, 3);
        assert_eq!(cfg.default_max_tokens, 8000);
        assert_eq!(cfg.default_max_iterations, 12);
        assert_eq!(cfg.default_timeout_ms, 5000);
        clear_subagent_env();
    }

    #[test]
    fn from_env_surfaces_parse_error() {
        let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_subagent_env();
        std::env::set_var(AURA_SUBAGENT_MAX_DEPTH, "not-a-number");
        let err = SubagentConfig::from_env().expect_err("invalid value must surface");
        let msg = format!("{err}");
        assert!(
            msg.contains(AURA_SUBAGENT_MAX_DEPTH),
            "error mentions env name: {msg}"
        );
        clear_subagent_env();
    }
}
