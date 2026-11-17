//! Run-trace capture level and the harness parameter snapshot recorded
//! with every run.

use serde::{Deserialize, Serialize};

use crate::env::{lookup_string, AURA_TRACE};
use crate::AuraConfig;

/// How much of a run the trace recorder keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceLevel {
    /// No bundle is written.
    Off,
    /// Steps, decisions, usage and rewards, without request blobs.
    #[default]
    Decisions,
    /// Everything in `Decisions` plus the exact model requests,
    /// responses and thinking text as content-addressed blobs.
    Full,
}

impl TraceLevel {
    /// Parse `off` / `decisions` / `full` (case-insensitive). Unknown
    /// values return `None`.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" | "none" => Some(Self::Off),
            "decisions" | "on" | "1" | "true" | "summary" => Some(Self::Decisions),
            "full" | "blobs" => Some(Self::Full),
            _ => None,
        }
    }

    /// Stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Decisions => "decisions",
            Self::Full => "full",
        }
    }
}

/// Run-trace settings.
#[derive(Debug, Clone, Copy, Default)]
pub struct TraceConfig {
    /// env: `AURA_TRACE` (default: `decisions`)
    ///
    /// Capture level for run bundles under `AURA_HOME/runs/`.
    pub level: TraceLevel,
    /// Whether `AURA_TRACE` was set. The interactive terminal records
    /// by default; the gateway, which serves many users, records only
    /// when the operator set it.
    pub explicit: bool,
}

impl TraceConfig {
    /// Compile-time defaults.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            level: TraceLevel::Decisions,
            explicit: false,
        }
    }

    /// Apply env overrides. Unknown `AURA_TRACE` values keep the
    /// default so a typo never disables tracing silently.
    #[must_use]
    pub fn from_env() -> Self {
        let mut cfg = Self::defaults();
        if let Some(level) = lookup_string(AURA_TRACE)
            .as_deref()
            .and_then(TraceLevel::parse)
        {
            cfg.level = level;
            cfg.explicit = true;
        }
        cfg
    }
}

/// Every tunable harness heuristic threshold, as one serializable
/// snapshot. Recorded in each run's metadata and in the policy of every
/// harness step, and applied per arm by evals and run branching.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HarnessParams {
    /// Whether the `implement_now` nudge can fire.
    pub implement_now_enabled: bool,
    /// Exploration calls before `implement_now` fires.
    pub implement_now_threshold: usize,
    /// Whether exploration is blocked after `implement_now` until a write.
    pub implement_now_hard_block: bool,
    /// Identical read results before the repeated-read nudge.
    pub repeated_read_threshold: usize,
    /// Identical failed edits before the stop-repeating warning.
    pub failed_edit_warn_after: u32,
    /// Identical failed edits before the turn stops as stalled.
    pub failed_edit_stop_after: u32,
    /// Fresh reads granted for a path after writing it.
    pub reads_after_write_allowance: u8,
    /// Whether context compaction is disabled.
    pub compaction_disabled: bool,
    /// Context utilization that triggers history compaction.
    pub compaction_tier_history: f64,
    /// Context utilization that triggers aggressive compaction.
    pub compaction_tier_aggressive: f64,
    /// Context utilization for the 60% compaction tier.
    pub compaction_tier_60: f64,
    /// Context utilization for the 30% compaction tier.
    pub compaction_tier_30: f64,
    /// Context utilization for micro compaction.
    pub compaction_tier_micro: f64,
    /// Iteration utilization for the first budget warning.
    pub budget_warning_30: f64,
    /// Iteration utilization for the no-write budget warning.
    pub budget_warning_40_no_write: f64,
    /// Iteration utilization for the final budget warning.
    pub budget_warning_60: f64,
    /// Iterations between automatic build checks.
    pub auto_build_cooldown: usize,
}

impl Default for HarnessParams {
    fn default() -> Self {
        Self::from_config(&AuraConfig::defaults())
    }
}

impl HarnessParams {
    /// Snapshot the thresholds from `cfg`.
    #[must_use]
    pub fn from_config(cfg: &AuraConfig) -> Self {
        let a = &cfg.agent;
        Self {
            implement_now_enabled: a.steering.implement_now_enabled,
            implement_now_threshold: a.steering.implement_now_threshold,
            implement_now_hard_block: a.steering.implement_now_hard_block,
            repeated_read_threshold: a.steering.repeated_read_threshold,
            failed_edit_warn_after: a.steering.failed_edit_warn_after,
            failed_edit_stop_after: a.steering.failed_edit_stop_after,
            reads_after_write_allowance: a.tools.reads_after_write_allowance,
            compaction_disabled: a.compaction.disabled,
            compaction_tier_history: a.compaction.tier_history,
            compaction_tier_aggressive: a.compaction.tier_aggressive,
            compaction_tier_60: a.compaction.tier_60,
            compaction_tier_30: a.compaction.tier_30,
            compaction_tier_micro: a.compaction.tier_micro,
            budget_warning_30: a.thinking.budget_warning_30,
            budget_warning_40_no_write: a.thinking.budget_warning_40_no_write,
            budget_warning_60: a.thinking.budget_warning_60,
            auto_build_cooldown: a.thinking.auto_build_cooldown,
        }
    }

    /// Thresholds of the currently installed config.
    #[must_use]
    pub fn current() -> Self {
        Self::from_config(&crate::current())
    }

    /// Write these thresholds into `cfg`.
    pub fn apply_to(&self, cfg: &mut AuraConfig) {
        let a = &mut cfg.agent;
        a.steering.implement_now_enabled = self.implement_now_enabled;
        a.steering.implement_now_threshold = self.implement_now_threshold;
        a.steering.implement_now_hard_block = self.implement_now_hard_block;
        a.steering.repeated_read_threshold = self.repeated_read_threshold;
        a.steering.failed_edit_warn_after = self.failed_edit_warn_after;
        a.steering.failed_edit_stop_after = self.failed_edit_stop_after;
        a.tools.reads_after_write_allowance = self.reads_after_write_allowance;
        a.compaction.disabled = self.compaction_disabled;
        a.compaction.tier_history = self.compaction_tier_history;
        a.compaction.tier_aggressive = self.compaction_tier_aggressive;
        a.compaction.tier_60 = self.compaction_tier_60;
        a.compaction.tier_30 = self.compaction_tier_30;
        a.compaction.tier_micro = self.compaction_tier_micro;
        a.thinking.budget_warning_30 = self.budget_warning_30;
        a.thinking.budget_warning_40_no_write = self.budget_warning_40_no_write;
        a.thinking.budget_warning_60 = self.budget_warning_60;
        a.thinking.auto_build_cooldown = self.auto_build_cooldown;
    }

    /// Install these thresholds into the process-wide config. Meant for
    /// process start (eval arms, `aura runs branch --params`), before
    /// any agent loop runs.
    pub fn install(&self) {
        let mut cfg = crate::current();
        self.apply_to(&mut cfg);
        crate::install(cfg);
    }

    /// Parse a partial JSON object over the current thresholds. Keys
    /// that are not present keep their current value.
    ///
    /// # Errors
    ///
    /// Returns the JSON error when `overrides` is not an object of
    /// known fields with the right types.
    pub fn with_overrides(&self, overrides: &serde_json::Value) -> serde_json::Result<Self> {
        let mut base = serde_json::to_value(self)?;
        if let (Some(base), Some(over)) = (base.as_object_mut(), overrides.as_object()) {
            for (k, v) in over {
                base.insert(k.clone(), v.clone());
            }
        }
        serde_json::from_value(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trace_levels() {
        assert_eq!(TraceLevel::parse("OFF"), Some(TraceLevel::Off));
        assert_eq!(TraceLevel::parse("full"), Some(TraceLevel::Full));
        assert_eq!(TraceLevel::parse("decisions"), Some(TraceLevel::Decisions));
        assert_eq!(TraceLevel::parse("bogus"), None);
    }

    #[test]
    fn params_roundtrip_through_config() {
        let mut cfg = AuraConfig::defaults();
        let mut params = HarnessParams::from_config(&cfg);
        params.implement_now_threshold = 4;
        params.compaction_tier_60 = 0.5;
        params.apply_to(&mut cfg);
        assert_eq!(HarnessParams::from_config(&cfg), params);
    }

    #[test]
    fn overrides_merge_over_base() {
        let base = HarnessParams::default();
        let arm = base
            .with_overrides(&serde_json::json!({"implement_now_threshold": 3}))
            .unwrap();
        assert_eq!(arm.implement_now_threshold, 3);
        assert_eq!(arm.repeated_read_threshold, base.repeated_read_threshold);
        assert!(base
            .with_overrides(&serde_json::json!({"implement_now_threshold": "x"}))
            .is_err());
    }
}
