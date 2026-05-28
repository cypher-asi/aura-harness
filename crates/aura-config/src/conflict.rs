//! Domain-scoped conflict-lock knobs for the future `aura-exec-conflict`
//! crate (Phase 4a).
//!
//! ## Invariants ([rules.md §13])
//!
//! - `default_wait_ms` bounds blocking time for any conflict-lock
//!   acquisition; `0` means "try-once" (acquire-or-fail).
//! - `enabled = false` means conflict locks are skipped entirely.
//!   The advisory layer expects this knob to be honoured even when
//!   the lock store itself is reachable.
//! - All env reads are non-fatal; a malformed override returns
//!   [`crate::ConfigError`] without poisoning the process slot.
//!
//! ## Owned env vars
//!
//! | Var | Type | Default | Field |
//! | --- | --- | --- | --- |
//! | `AURA_CONFLICT_ENABLED` | bool | `true` | [`ConflictConfig::enabled`] |
//! | `AURA_CONFLICT_DEFAULT_WAIT_MS` | u64 | `30_000` | [`ConflictConfig::default_wait_ms`] |

use serde::{Deserialize, Serialize};

use crate::env::{
    lookup_bool, lookup_numeric, AURA_CONFLICT_DEFAULT_WAIT_MS, AURA_CONFLICT_ENABLED,
    FALSY_LITERALS, TRUTHY_LITERALS,
};

const DEFAULT_ENABLED: bool = true;
const DEFAULT_WAIT_MS: u64 = 30_000;

/// Conflict-lock knobs. See the module-level docs for invariants.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "snake_case")]
pub struct ConflictConfig {
    /// Master enable flag. When `false`, the advisory layer skips
    /// every acquisition entirely.
    pub enabled: bool,
    /// Default wait budget (ms) for a conflict-lock acquire call.
    /// `0` = try-once.
    pub default_wait_ms: u64,
}

impl ConflictConfig {
    /// Compile-time defaults. No env access.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            enabled: DEFAULT_ENABLED,
            default_wait_ms: DEFAULT_WAIT_MS,
        }
    }

    /// Apply env overrides.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ConfigError`] when
    /// `AURA_CONFLICT_DEFAULT_WAIT_MS` is non-empty but unparseable.
    pub fn from_env() -> Result<Self, crate::ConfigError> {
        let mut cfg = Self::defaults();
        cfg.enabled = lookup_bool(
            AURA_CONFLICT_ENABLED,
            DEFAULT_ENABLED,
            TRUTHY_LITERALS,
            FALSY_LITERALS,
        );
        if let Some(v) = lookup_numeric::<u64>(AURA_CONFLICT_DEFAULT_WAIT_MS)? {
            cfg.default_wait_ms = v;
        }
        Ok(cfg)
    }
}

impl Default for ConflictConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::ENV_TEST_LOCK;

    fn clear_conflict_env() {
        std::env::remove_var(AURA_CONFLICT_ENABLED);
        std::env::remove_var(AURA_CONFLICT_DEFAULT_WAIT_MS);
    }

    #[test]
    fn defaults_are_stable() {
        let cfg = ConflictConfig::defaults();
        assert!(cfg.enabled);
        assert_eq!(cfg.default_wait_ms, DEFAULT_WAIT_MS);
    }

    #[test]
    fn defaults_are_const_evaluable() {
        const _DEFAULTS: ConflictConfig = ConflictConfig::defaults();
    }

    #[test]
    fn from_env_falls_back_to_defaults_when_unset() {
        let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_conflict_env();
        let cfg = ConflictConfig::from_env().expect("defaults must parse");
        assert_eq!(cfg, ConflictConfig::defaults());
        clear_conflict_env();
    }

    #[test]
    fn from_env_applies_overrides() {
        let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_conflict_env();
        std::env::set_var(AURA_CONFLICT_ENABLED, "false");
        std::env::set_var(AURA_CONFLICT_DEFAULT_WAIT_MS, "0");
        let cfg = ConflictConfig::from_env().expect("override must parse");
        assert!(!cfg.enabled);
        assert_eq!(cfg.default_wait_ms, 0);
        clear_conflict_env();
    }
}
