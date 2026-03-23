//! High-level agent execution: agentic task, chat, and shell-task runners.
//!
//! `AgentRunner` combines task context setup, agent loop configuration, and
//! result processing into a convenient orchestration layer built on top of
//! [`AgentLoop`].

use std::path::Path;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use aura_reasoner::{Message, ModelProvider, ToolDefinition};

use crate::agent_loop::{AgentLoop, AgentLoopConfig};
use crate::events::AgentLoopEvent;
use crate::file_ops::FileOp;
use crate::policy::{
    classify_task_complexity, compute_exploration_allowance, compute_thinking_budget,
    resolve_simple_model, TaskComplexity,
};
use crate::prompts::{
    agentic_execution_system_prompt, build_agentic_task_context, build_chat_system_prompt,
    AgentInfo, ProjectInfo, SessionInfo, SpecInfo, TaskInfo,
};
use crate::task_context;
use crate::types::{AgentLoopResult, AgentToolExecutor};
use crate::verify::{
    auto_correct_build_command, normalize_error_signature, run_build_command, BuildFixAttemptRecord,
};

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// Result of executing an agentic task.
#[derive(Debug, Clone, Default)]
pub struct TaskExecutionResult {
    pub notes: String,
    pub file_ops: Vec<FileOp>,
    pub follow_up_tasks: Vec<FollowUpSuggestion>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub files_already_applied: bool,
}

/// Suggested follow-up task from agent execution.
#[derive(Debug, Clone)]
pub struct FollowUpSuggestion {
    pub title: String,
    pub description: String,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the `AgentRunner`.
#[derive(Debug, Clone)]
pub struct AgentRunnerConfig {
    pub max_agentic_iterations: usize,
    pub max_shell_task_retries: u32,
    pub task_execution_max_tokens: u32,
    pub thinking_budget: u32,
    pub stream_timeout_secs: u64,
    pub max_context_tokens: u64,
    pub max_task_credits: Option<u64>,
    pub default_model: String,
    pub simple_model: String,
}

impl Default for AgentRunnerConfig {
    fn default() -> Self {
        Self {
            max_agentic_iterations: 40,
            max_shell_task_retries: 4,
            task_execution_max_tokens: 16_384,
            thinking_budget: 10_000,
            stream_timeout_secs: 120,
            max_context_tokens: 200_000,
            max_task_credits: None,
            default_model: "claude-opus-4-6-20250514".to_string(),
            simple_model: "claude-sonnet-4-20250514".to_string(),
        }
    }
}

/// Parameters for running an agentic task.
pub struct AgenticTaskParams<'a> {
    pub project: &'a ProjectInfo<'a>,
    pub spec: &'a SpecInfo<'a>,
    pub task: &'a TaskInfo<'a>,
    pub session: &'a SessionInfo<'a>,
    pub agent: Option<&'a AgentInfo<'a>>,
    pub work_log: &'a [String],
    pub completed_deps: &'a [TaskInfo<'a>],
    pub workspace_map: &'a str,
    pub codebase_snapshot: &'a str,
    pub type_defs_context: &'a str,
    pub dep_api_context: &'a str,
    pub member_count: usize,
    pub tools: Vec<ToolDefinition>,
}

/// Context for a shell task execution.
pub struct ShellTaskParams<'a> {
    pub command: &'a str,
    pub project_root: &'a Path,
}

// ---------------------------------------------------------------------------
// AgentRunner
// ---------------------------------------------------------------------------

/// High-level runner that configures and executes agent loops for tasks,
/// chat sessions, and shell commands.
pub struct AgentRunner {
    pub config: AgentRunnerConfig,
}

impl AgentRunner {
    #[must_use]
    pub fn new(config: AgentRunnerConfig) -> Self {
        Self { config }
    }

    /// Execute an agentic task: build context, configure the loop, run it,
    /// and process results.
    pub async fn execute_task(
        &self,
        provider: &dyn ModelProvider,
        executor: &dyn AgentToolExecutor,
        params: &AgenticTaskParams<'_>,
        event_tx: Option<mpsc::UnboundedSender<AgentLoopEvent>>,
        cancel: Option<CancellationToken>,
    ) -> Result<TaskExecutionResult, crate::AgentError> {
        let complexity = classify_task_complexity(params.task.title, params.task.description);

        let exploration_allowance = compute_exploration_allowance(
            params.task.title,
            params.task.description,
            params.member_count,
        );

        let workspace_info = if params.workspace_map.is_empty() {
            None
        } else {
            Some(params.workspace_map)
        };
        let system_prompt = agentic_execution_system_prompt(
            params.project,
            params.agent,
            workspace_info,
            exploration_allowance,
        );

        let work_log_summary = task_context::build_work_log_summary(params.work_log);
        let base_context = build_agentic_task_context(
            params.project,
            params.spec,
            params.task,
            params.session,
            params.completed_deps,
            &work_log_summary,
        );
        let task_ctx = task_context::build_full_task_context(
            base_context,
            params.workspace_map,
            params.type_defs_context,
            params.codebase_snapshot,
            params.dep_api_context,
        );

        let loop_config = configure_loop_config(
            complexity,
            &self.config,
            exploration_allowance,
            params.member_count,
            system_prompt,
        );

        let agent_loop = AgentLoop::new(loop_config);
        let messages = vec![Message::user(&task_ctx)];

        let result = agent_loop
            .run_with_events(
                provider,
                executor,
                messages,
                params.tools.clone(),
                event_tx,
                cancel,
            )
            .await
            .map_err(|e| crate::AgentError::Internal(e.to_string()))?;

        Ok(finalize_loop_result(result))
    }

    /// Execute a chat interaction using the agent loop.
    pub async fn execute_chat(
        &self,
        provider: &dyn ModelProvider,
        executor: &dyn AgentToolExecutor,
        project: &ProjectInfo<'_>,
        custom_system_prompt: &str,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        event_tx: Option<mpsc::UnboundedSender<AgentLoopEvent>>,
        cancel: Option<CancellationToken>,
    ) -> Result<AgentLoopResult, crate::AgentError> {
        let system_prompt = {
            let name = project.name.to_owned();
            let description = project.description.to_owned();
            let folder_path = project.folder_path.to_owned();
            let build_command = project.build_command.map(str::to_owned);
            let test_command = project.test_command.map(str::to_owned);
            let custom = custom_system_prompt.to_owned();
            tokio::task::spawn_blocking(move || {
                let p = ProjectInfo {
                    name: &name,
                    description: &description,
                    folder_path: &folder_path,
                    build_command: build_command.as_deref(),
                    test_command: test_command.as_deref(),
                };
                build_chat_system_prompt(&p, &custom)
            })
            .await
            .map_err(|e| crate::AgentError::Internal(e.to_string()))?
        };
        let config = AgentLoopConfig {
            system_prompt,
            model: self.config.default_model.clone(),
            max_tokens: self.config.task_execution_max_tokens,
            stream_timeout: Duration::from_secs(self.config.stream_timeout_secs),
            billing_reason: "aura_chat".to_string(),
            max_context_tokens: Some(self.config.max_context_tokens),
            ..AgentLoopConfig::default()
        };
        let agent_loop = AgentLoop::new(config);
        agent_loop
            .run_with_events(provider, executor, messages, tools, event_tx, cancel)
            .await
            .map_err(|e| crate::AgentError::Internal(e.to_string()))
    }

    /// Execute a shell task with automatic retry on failure.
    pub async fn execute_shell_task(
        &self,
        params: &ShellTaskParams<'_>,
        event_tx: Option<&mpsc::UnboundedSender<AgentLoopEvent>>,
    ) -> Result<TaskExecutionResult, crate::AgentError> {
        let command = auto_correct_build_command(params.command)
            .unwrap_or_else(|| params.command.to_string());
        let max_attempts = self.config.max_shell_task_retries;
        let mut prior: Vec<BuildFixAttemptRecord> = Vec::new();

        for attempt in 1..=max_attempts {
            if let Some(tx) = event_tx {
                let _ = tx.send(AgentLoopEvent::TextDelta(format!(
                    "Running: {} (attempt {attempt}/{max_attempts})\n",
                    command,
                )));
            }

            let result = run_build_command(params.project_root, &command, None).await
                .map_err(|e| crate::AgentError::BuildFailed(e.to_string()))?;

            if result.success {
                let notes = format!(
                    "Command `{}` succeeded on attempt {attempt}.\n{}",
                    command, result.stdout,
                );
                if let Some(tx) = event_tx {
                    let _ = tx.send(AgentLoopEvent::TextDelta(notes.clone()));
                }
                return Ok(TaskExecutionResult {
                    notes,
                    files_already_applied: false,
                    ..TaskExecutionResult::default()
                });
            }

            let detail = if !result.stderr.is_empty() {
                &result.stderr
            } else {
                &result.stdout
            };

            if let Some(tx) = event_tx {
                let _ = tx.send(AgentLoopEvent::TextDelta(format!(
                    "Command failed (attempt {attempt}):\n{detail}\n",
                )));
            }

            if let Some(err) = check_repeated_error(
                &prior,
                &normalize_error_signature(detail),
                attempt,
                &command,
            ) {
                return Err(crate::AgentError::BuildFailed(err.to_string()));
            }

            if attempt < max_attempts {
                prior.push(BuildFixAttemptRecord {
                    stderr: detail.to_string(),
                    error_signature: normalize_error_signature(detail),
                    files_changed: Vec::new(),
                    changes_summary: String::new(),
                });
            }
        }

        Err(crate::AgentError::BuildFailed(format!("command `{command}` failed after {max_attempts} attempts")))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an [`AgentLoopConfig`] from task complexity and runner config.
pub fn configure_loop_config(
    complexity: TaskComplexity,
    config: &AgentRunnerConfig,
    exploration_allowance: usize,
    member_count: usize,
    system_prompt: String,
) -> AgentLoopConfig {
    let thinking_budget = match complexity {
        TaskComplexity::Simple => 2_000.min(config.thinking_budget),
        TaskComplexity::Standard => compute_thinking_budget(config.thinking_budget, member_count),
        TaskComplexity::Complex => {
            compute_thinking_budget(config.thinking_budget, member_count).max(12_000)
        }
    };
    let max_tokens = match complexity {
        TaskComplexity::Simple => config.task_execution_max_tokens.min(8_192),
        _ => config.task_execution_max_tokens,
    };
    let max_iterations = match complexity {
        TaskComplexity::Simple => config.max_agentic_iterations.min(15),
        _ => config.max_agentic_iterations,
    };
    let model = match complexity {
        TaskComplexity::Simple => resolve_simple_model(&config.simple_model),
        _ => config.default_model.clone(),
    };

    // The thinking_budget from policy feeds into the loop's initial thinking state
    // via max_tokens; the AgentLoop tapers it across iterations.
    let _ = thinking_budget;

    AgentLoopConfig {
        max_iterations,
        max_tokens,
        stream_timeout: Duration::from_secs(config.stream_timeout_secs),
        billing_reason: "aura_task".to_string(),
        max_context_tokens: Some(config.max_context_tokens),
        credit_budget: config.max_task_credits,
        exploration_allowance,
        auto_build_cooldown: 1,
        system_prompt,
        model,
        ..AgentLoopConfig::default()
    }
}

/// Process an [`AgentLoopResult`] into a [`TaskExecutionResult`].
fn finalize_loop_result(result: AgentLoopResult) -> TaskExecutionResult {
    let notes = if result.total_text.is_empty() {
        "Task completed via agentic tool-use loop".to_string()
    } else {
        result.total_text
    };
    TaskExecutionResult {
        notes,
        file_ops: Vec::new(),
        follow_up_tasks: Vec::new(),
        input_tokens: result.total_input_tokens,
        output_tokens: result.total_output_tokens,
        files_already_applied: true,
    }
}

/// Check if the same error signature is repeating across fix attempts.
///
/// Returns an error if the same pattern has appeared 3+ consecutive times.
pub fn check_repeated_error(
    prior: &[BuildFixAttemptRecord],
    current_sig: &str,
    attempt: u32,
    command: &str,
) -> Option<anyhow::Error> {
    let consecutive_dupes = prior
        .iter()
        .rev()
        .take_while(|a| a.error_signature == current_sig)
        .count();
    if consecutive_dupes >= 2 {
        tracing::info!(
            attempt,
            "same shell error pattern repeated {} times, aborting fix loop",
            consecutive_dupes + 1,
        );
        return Some(anyhow::anyhow!(
            "command `{command}` keeps failing with the same error after {} attempts",
            consecutive_dupes + 1,
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_loop_config_simple_caps_max_tokens() {
        let config = AgentRunnerConfig::default();
        let loop_cfg =
            configure_loop_config(TaskComplexity::Simple, &config, 8, 3, "system".into());
        assert!(loop_cfg.max_tokens <= 8_192);
        assert!(loop_cfg.max_iterations <= 15);
    }

    #[test]
    fn configure_loop_config_complex_uses_full_budget() {
        let config = AgentRunnerConfig::default();
        let loop_cfg =
            configure_loop_config(TaskComplexity::Complex, &config, 18, 3, "system".into());
        assert_eq!(loop_cfg.max_tokens, config.task_execution_max_tokens);
        assert_eq!(loop_cfg.max_iterations, config.max_agentic_iterations);
    }

    #[test]
    fn configure_loop_config_maps_all_fields() {
        let config = AgentRunnerConfig::default();
        let loop_cfg =
            configure_loop_config(TaskComplexity::Standard, &config, 12, 3, "system".into());
        assert_eq!(loop_cfg.exploration_allowance, 12);
        assert_eq!(loop_cfg.billing_reason, "aura_task");
        assert_eq!(loop_cfg.auto_build_cooldown, 1);
    }

    #[test]
    fn check_repeated_error_returns_none_on_first() {
        let result = check_repeated_error(&[], "sig1", 1, "cargo build");
        assert!(result.is_none());
    }

    #[test]
    fn check_repeated_error_triggers_after_three_dupes() {
        let prior = vec![
            BuildFixAttemptRecord {
                stderr: "err".into(),
                error_signature: "sig1".into(),
                files_changed: vec![],
                changes_summary: String::new(),
            },
            BuildFixAttemptRecord {
                stderr: "err".into(),
                error_signature: "sig1".into(),
                files_changed: vec![],
                changes_summary: String::new(),
            },
        ];
        let result = check_repeated_error(&prior, "sig1", 3, "cargo build");
        assert!(result.is_some());
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
}
