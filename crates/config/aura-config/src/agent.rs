//! Top-level agent config wrapper + the agent-specific constants that
//! aren't worth a dedicated submodule (task-context budgets,
//! pervasive-error window, `AURA_SIMPLE_MODEL` override).

use aura_core_modes::AgentMode;

use crate::env::{lookup_string, AURA_DEFAULT_MODE, AURA_FLEET_DEFAULT_MODE, AURA_SIMPLE_MODEL};
use crate::{
    AutomatonConfig, CompactionConfig, PromptsConfig, SteeringConfig, ThinkingConfig, ToolsConfig,
    VerifyConfig,
};

// ---------------------------------------------------------------------------
// Compile-time constants
// ---------------------------------------------------------------------------

/// Maximum tool-use iterations before the loop terminates.
///
/// Derived from [`aura_core_types::MAX_TURNS`] — the single source of truth
/// for every "max turns / max iterations" knob in the system.
pub const MAX_ITERATIONS: usize = aura_core_types::MAX_TURNS as usize;

/// Maximum characters retained in the full task-context blob assembled
/// by `aura_agent::task_context::build_full_task_context`.
pub const MAX_TASK_CONTEXT_CHARS: usize = 160_000;

/// Default cap on the bootstrap task-context (the slice that ships
/// with the first model request). Overridable via
/// `AURA_AGENT_BOOTSTRAP_CONTEXT_CHARS`, parsed through
/// [`crate::PromptsConfig::bootstrap_context_chars`].
pub const DEFAULT_BOOTSTRAP_TASK_CONTEXT_CHARS: usize = 12_000;

/// Maximum characters retained when summarizing the per-task work log
/// for inclusion in the task context.
pub const MAX_WORK_LOG_TASK_CONTEXT: usize = 4_000;

/// Rolling window size for the recent-tool-outcome buffer consumed by
/// the pervasive-error guard on `task_done`.
pub const RECENT_OUTCOMES_WINDOW: usize = 16;

/// Minimum tool calls in the rolling window before the pervasive-error
/// guard is allowed to fire.
pub const PERVASIVE_ERROR_MIN_CALLS: usize = 6;

/// Error ratio (0.0 .. 1.0) at which the pervasive-error guard
/// rejects a `task_done` call.
pub const PERVASIVE_ERROR_THRESHOLD: f64 = 0.7;

// ---------------------------------------------------------------------------
// AgentConfig
// ---------------------------------------------------------------------------

/// Agent-layer config root. Groups the eight specialised sub-configs
/// plus the agent-specific constants and the `AURA_SIMPLE_MODEL`
/// override.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Loop-level constants (iterations, task-context budgets,
    /// pervasive-error window). See `loop_` field comments for the
    /// individual knobs.
    pub loop_: LoopConfig,
    /// Tool pipeline configuration (cacheable / exploration / write
    /// tool lists, write-cap bytes, heartbeat cadence).
    pub tools: ToolsConfig,
    /// Thinking-budget taper, auto-build cooldown, budget warnings.
    pub thinking: ThinkingConfig,
    /// Compaction kill switch + tier thresholds.
    pub compaction: CompactionConfig,
    /// `implement_now` / repeated-read steering knobs.
    pub steering: SteeringConfig,
    /// Prompt-layer knobs (bootstrap byte budgets, repeated-read
    /// display chars, compaction summary block cap).
    pub prompts: PromptsConfig,
    /// Verify / build-runner / git read-only knobs.
    pub verify: VerifyConfig,
    /// Automaton-builtin knobs (`spec_gen` / `task_refinement` token
    /// caps, dev-loop retry-note budget).
    pub automaton: AutomatonConfig,
    /// env: `AURA_SIMPLE_MODEL` (default: `None`)
    ///
    /// Operator override for the model used on tasks classified as
    /// `TaskComplexity::Simple`. Consumed by
    /// `aura_agent::turn_config::resolve_simple_model`. Empty /
    /// whitespace-only values are treated as unset.
    pub simple_model_override: Option<String>,
    /// env: `AURA_DEFAULT_MODE`, or the older `AURA_FLEET_DEFAULT_MODE`
    /// (default: `agent`)
    ///
    /// Mode for sessions that don't request one; the daemon rung of the
    /// [`AgentMode`] resolution chain (see [`crate::session_mode`]).
    pub default_mode: AgentMode,
}

impl AgentConfig {
    /// Compile-time defaults.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            loop_: LoopConfig::defaults(),
            tools: ToolsConfig::defaults(),
            thinking: ThinkingConfig::defaults(),
            compaction: CompactionConfig::defaults(),
            steering: SteeringConfig::defaults(),
            prompts: PromptsConfig::defaults(),
            verify: VerifyConfig::defaults(),
            automaton: AutomatonConfig::defaults(),
            simple_model_override: None,
            default_mode: AgentMode::Agent,
        }
    }

    /// Apply env overrides across every sub-config.
    ///
    /// # Errors
    ///
    /// Returns the first [`crate::ConfigError`] surfaced by a numeric
    /// override that was set but unparseable.
    pub fn from_env() -> Result<Self, crate::ConfigError> {
        Ok(Self {
            loop_: LoopConfig::defaults(),
            tools: ToolsConfig::from_env()?,
            thinking: ThinkingConfig::defaults(),
            compaction: CompactionConfig::from_env()?,
            steering: SteeringConfig::from_env()?,
            prompts: PromptsConfig::from_env()?,
            verify: VerifyConfig::from_env()?,
            automaton: AutomatonConfig::defaults(),
            simple_model_override: lookup_string(AURA_SIMPLE_MODEL),
            default_mode: default_mode_from_env()?,
        })
    }
}

fn default_mode_from_env() -> Result<AgentMode, crate::ConfigError> {
    let Some((name, raw)) = lookup_string(AURA_DEFAULT_MODE)
        .map(|raw| (AURA_DEFAULT_MODE, raw))
        .or_else(|| {
            lookup_string(AURA_FLEET_DEFAULT_MODE).map(|raw| (AURA_FLEET_DEFAULT_MODE, raw))
        })
    else {
        return Ok(AgentMode::Agent);
    };
    match raw.to_lowercase().as_str() {
        "agent" => Ok(AgentMode::Agent),
        "plan" => Ok(AgentMode::Plan),
        "ask" => Ok(AgentMode::Ask),
        "debug" => Ok(AgentMode::Debug),
        _ => Err(crate::ConfigError::InvalidValue {
            name,
            raw,
            message: "expected one of agent|plan|ask|debug".to_string(),
        }),
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

// ---------------------------------------------------------------------------
// LoopConfig
// ---------------------------------------------------------------------------

/// Loop-level pacing constants and the pervasive-error window.
///
/// All values are compile-time today. The struct exists so call sites
/// can read them through `aura_config::agent().loop_.<field>` instead
/// of importing each `pub const` individually.
#[derive(Debug, Clone, Copy)]
pub struct LoopConfig {
    /// See [`MAX_ITERATIONS`].
    pub max_iterations: usize,
    /// See [`MAX_TASK_CONTEXT_CHARS`].
    pub max_task_context_chars: usize,
    /// See [`DEFAULT_BOOTSTRAP_TASK_CONTEXT_CHARS`].
    pub default_bootstrap_task_context_chars: usize,
    /// See [`MAX_WORK_LOG_TASK_CONTEXT`].
    pub max_work_log_task_context: usize,
    /// See [`RECENT_OUTCOMES_WINDOW`].
    pub recent_outcomes_window: usize,
    /// See [`PERVASIVE_ERROR_MIN_CALLS`].
    pub pervasive_error_min_calls: usize,
    /// See [`PERVASIVE_ERROR_THRESHOLD`].
    pub pervasive_error_threshold: f64,
}

impl LoopConfig {
    /// Compile-time defaults.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            max_iterations: MAX_ITERATIONS,
            max_task_context_chars: MAX_TASK_CONTEXT_CHARS,
            default_bootstrap_task_context_chars: DEFAULT_BOOTSTRAP_TASK_CONTEXT_CHARS,
            max_work_log_task_context: MAX_WORK_LOG_TASK_CONTEXT,
            recent_outcomes_window: RECENT_OUTCOMES_WINDOW,
            pervasive_error_min_calls: PERVASIVE_ERROR_MIN_CALLS,
            pervasive_error_threshold: PERVASIVE_ERROR_THRESHOLD,
        }
    }
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::ENV_TEST_LOCK;

    fn clear_mode_env() {
        std::env::remove_var(AURA_DEFAULT_MODE);
        std::env::remove_var(AURA_FLEET_DEFAULT_MODE);
    }

    #[test]
    fn default_mode_is_agent_when_unset() {
        let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_mode_env();
        assert_eq!(default_mode_from_env().unwrap(), AgentMode::Agent);
    }

    #[test]
    fn default_mode_parses_and_prefers_the_new_name() {
        let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_mode_env();
        std::env::set_var(AURA_FLEET_DEFAULT_MODE, "plan");
        assert_eq!(default_mode_from_env().unwrap(), AgentMode::Plan);
        std::env::set_var(AURA_DEFAULT_MODE, "ask");
        assert_eq!(default_mode_from_env().unwrap(), AgentMode::Ask);
        clear_mode_env();
    }

    #[test]
    fn default_mode_rejects_unknown_values() {
        let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_mode_env();
        std::env::set_var(AURA_DEFAULT_MODE, "yolo");
        assert!(matches!(
            default_mode_from_env(),
            Err(crate::ConfigError::InvalidValue { .. })
        ));
        clear_mode_env();
    }
}
