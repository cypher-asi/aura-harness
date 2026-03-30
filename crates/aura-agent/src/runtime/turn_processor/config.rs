//! Configuration types for the turn processor.

use std::path::PathBuf;

/// Turn processor configuration.
#[derive(Debug, Clone)]
pub struct TurnConfig {
    /// Maximum steps (model calls) per turn
    pub max_steps: u32,
    /// Maximum tool calls per step
    pub max_tool_calls_per_step: u32,
    /// Model timeout in milliseconds.
    pub model_timeout_ms: u64,
    /// Tool execution timeout in milliseconds.
    pub tool_timeout_ms: u64,
    /// Context window size (record entries)
    pub context_window: usize,
    /// Model to use
    pub model: String,
    /// System prompt
    pub system_prompt: String,
    /// Base workspace directory
    pub workspace_base: PathBuf,
    /// Whether we're in replay mode (skip model/tools)
    pub replay_mode: bool,
    /// Temperature for model calls
    pub temperature: Option<f32>,
    /// Max tokens per response
    pub max_tokens: u32,
    /// Context window size in tokens. When the estimated token count of
    /// `messages` exceeds `context_window_tokens * context_target_ratio`,
    /// older tool-result messages are truncated to stay within budget.
    pub context_window_tokens: usize,
    /// Target utilization ratio (0.0–1.0). Truncation triggers when
    /// estimated tokens exceed `context_window_tokens * context_target_ratio`.
    pub context_target_ratio: f32,
}

impl Default for TurnConfig {
    fn default() -> Self {
        Self {
            max_steps: 25,
            max_tool_calls_per_step: 8,
            model_timeout_ms: 180_000,
            tool_timeout_ms: 30_000,
            context_window: 50,
            model: crate::constants::DEFAULT_MODEL.to_string(),
            system_prompt: String::new(),
            workspace_base: PathBuf::from("./workspaces"),
            replay_mode: false,
            temperature: Some(0.2),
            max_tokens: 16_384,
            context_window_tokens: 200_000,
            context_target_ratio: 0.80,
        }
    }
}

/// Per-step configuration overrides.
///
/// Enables the caller (e.g., `AgentLoop`) to adjust behavior on a per-step
/// basis — for example, tapering the thinking budget after early iterations.
#[derive(Debug, Clone, Default)]
pub struct StepConfig {
    /// Override the thinking budget for this step. NOTE: not yet enforced by the turn processor.
    pub thinking_budget: Option<u32>,
    /// Override the model for this step.
    pub model_override: Option<String>,
    /// Override the maximum tool calls for this step. NOTE: not yet enforced by the turn processor.
    pub max_tool_calls: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_turn_config_defaults() {
        let config = TurnConfig::default();
        assert_eq!(config.max_steps, 25);
        assert_eq!(config.max_tool_calls_per_step, 8);
        assert_eq!(config.model_timeout_ms, 180_000);
        assert_eq!(config.tool_timeout_ms, 30_000);
        assert_eq!(config.context_window, 50);
        assert_eq!(config.model, crate::constants::DEFAULT_MODEL);
        assert!(!config.replay_mode);
        assert_eq!(config.temperature, Some(0.2));
        assert_eq!(config.max_tokens, 16_384);
        assert_eq!(config.context_window_tokens, 200_000);
        assert!((config.context_target_ratio - 0.80).abs() < f32::EPSILON);
    }

    #[test]
    fn test_turn_config_custom() {
        let config = TurnConfig {
            max_steps: 5,
            max_tool_calls_per_step: 2,
            model: "custom-model".to_string(),
            replay_mode: true,
            temperature: None,
            max_tokens: 8192,
            ..TurnConfig::default()
        };
        assert_eq!(config.max_steps, 5);
        assert_eq!(config.max_tool_calls_per_step, 2);
        assert_eq!(config.model, "custom-model");
        assert!(config.replay_mode);
        assert!(config.temperature.is_none());
        assert_eq!(config.max_tokens, 8192);
    }

    #[test]
    fn test_step_config_defaults() {
        let config = StepConfig::default();
        assert!(config.thinking_budget.is_none());
        assert!(config.model_override.is_none());
        assert!(config.max_tool_calls.is_none());
    }

    #[test]
    fn test_step_config_with_overrides() {
        let config = StepConfig {
            thinking_budget: Some(2048),
            model_override: Some("fast-model".to_string()),
            max_tool_calls: Some(4),
        };
        assert_eq!(config.thinking_budget, Some(2048));
        assert_eq!(config.model_override.as_deref(), Some("fast-model"));
        assert_eq!(config.max_tool_calls, Some(4));
    }

    #[test]
    fn test_turn_config_workspace_base() {
        let config = TurnConfig {
            workspace_base: PathBuf::from("/custom/path"),
            ..TurnConfig::default()
        };
        assert_eq!(config.workspace_base, PathBuf::from("/custom/path"));
    }

    #[test]
    fn test_default_system_prompt_empty() {
        let config = TurnConfig::default();
        assert!(config.system_prompt.is_empty());
    }
}
