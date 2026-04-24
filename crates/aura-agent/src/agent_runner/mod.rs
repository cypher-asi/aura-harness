//! High-level agent execution: agentic task, chat, and shell-task runners.
//!
//! `AgentRunner` combines task context setup, agent loop configuration, and
//! result processing into a convenient orchestration layer built on top of
//! [`AgentLoop`].

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

use aura_reasoner::{Message, ModelProvider, ToolDefinition};

use crate::agent_loop::{AgentLoop, AgentLoopConfig};
use crate::events::AgentLoopEvent;
use crate::file_ops::FileOp;
use crate::planning::TaskPhase;
use crate::prompts::{
    agentic_execution_system_prompt, build_agentic_task_context, build_chat_system_prompt,
    AgentInfo, ProjectInfo, SessionInfo, SpecInfo, TaskInfo,
};
use crate::task_context;
use crate::task_executor::TaskToolExecutor;
use crate::turn_config::{
    classify_task_complexity, compute_exploration_allowance, compute_thinking_budget,
    resolve_simple_model, TaskComplexity,
};
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
    /// When true, the agent explicitly declared that no file changes were
    /// required for this task. Automatons use this to distinguish legitimate
    /// no-op completions from false-positive successes.
    pub no_changes_needed: bool,
    /// When true, the agent progressed past the exploration phase and
    /// submitted a plan. Automatons use this to distinguish "never tried"
    /// from "tried but was interrupted" when `file_ops` is empty.
    pub reached_implementing: bool,
    /// Final message history from the agent loop. Downstream validators use
    /// this to build recovery hints (e.g. which file paths the agent tried
    /// to write before truncation).
    pub messages: Vec<aura_reasoner::Message>,
}

/// Suggested follow-up task from agent execution.
///
/// `Serialize` / `Deserialize` are derived because this type travels
/// through JSON execution-response parsing. The parser historically
/// carried its own copy of the struct and converted at the boundary,
/// which meant a field rename silently dropped the field on one side
/// of the copy. Phase 3 consolidated the two definitions here; Phase
/// 4e deleted the unused `parser` module.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// JWT auth token for proxy-mode LLM routing.
    pub auth_token: Option<String>,
}

impl Default for AgentRunnerConfig {
    fn default() -> Self {
        Self {
            max_agentic_iterations: 40,
            max_shell_task_retries: 4,
            task_execution_max_tokens: 16_384,
            thinking_budget: 10_000,
            // Matches the reasoner's default reqwest request timeout
            // (300s / `AURA_MODEL_TIMEOUT_MS`) so the outer `timeout()`
            // guard in `AgentLoop::call_model` does not preempt an
            // in-flight provider request. See the comment on
            // `AgentLoopConfig::stream_timeout`.
            stream_timeout_secs: 300,
            max_context_tokens: 200_000,
            max_task_credits: None,
            default_model: crate::constants::DEFAULT_MODEL.to_string(),
            simple_model: crate::constants::FALLBACK_MODEL.to_string(),
            auth_token: None,
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

/// Configuration for task-aware tracking in [`AgentRunner::execute_task_tracked`].
///
/// Bundles the inner tool executor and project metadata that `TaskToolExecutor`
/// needs so that callers do not have to construct the executor themselves.
pub struct TaskTrackingConfig {
    /// Inner executor that handles filesystem and search tools.
    pub inner_executor: Arc<dyn AgentToolExecutor>,
    /// Path to the project root for build and stub checks.
    pub project_folder: String,
    /// Build command (from project config or auto-detected).
    pub build_command: Option<String>,
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
    pub const fn new(config: AgentRunnerConfig) -> Self {
        Self { config }
    }

    /// Execute an agentic task: build context, configure the loop, run it,
    /// and process results.
    pub async fn execute_task(
        &self,
        provider: &dyn ModelProvider,
        executor: &dyn AgentToolExecutor,
        params: &AgenticTaskParams<'_>,
        event_tx: Option<mpsc::Sender<AgentLoopEvent>>,
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

        if let Some(ref llm_err) = result.llm_error {
            return Err(crate::AgentError::Internal(format!("LLM error: {llm_err}")));
        }
        if result.iterations == 0 {
            return Err(crate::AgentError::Internal(
                "Agent loop completed zero iterations — LLM may not be configured correctly".into(),
            ));
        }

        Ok(finalize_loop_result(result))
    }

    /// Execute an agentic task with built-in plan gating, file tracking,
    /// self-review, and stub detection.
    ///
    /// This is the preferred entry point for automatons: it internally
    /// constructs a [`TaskToolExecutor`] and merges its tracked state
    /// (file ops, notes, follow-ups, phase) into the returned result.
    pub async fn execute_task_tracked(
        &self,
        provider: &dyn ModelProvider,
        tracking: TaskTrackingConfig,
        params: &AgenticTaskParams<'_>,
        event_tx: Option<mpsc::Sender<AgentLoopEvent>>,
        cancel: Option<CancellationToken>,
    ) -> Result<TaskExecutionResult, crate::AgentError> {
        let task_executor = TaskToolExecutor {
            inner: tracking.inner_executor,
            project_folder: tracking.project_folder,
            build_command: tracking.build_command,
            task_context: String::new(),
            tracked_file_ops: Arc::default(),
            notes: Arc::default(),
            follow_ups: Arc::default(),
            stub_fix_attempts: Arc::default(),
            task_phase: Arc::new(Mutex::new(TaskPhase::Exploring)),
            self_review: Arc::default(),
            event_tx: event_tx.clone(),
            no_changes_needed: Arc::default(),
            recent_tool_outcomes: Arc::default(),
        };

        let mut result = self
            .execute_task(provider, &task_executor, params, event_tx, cancel)
            .await?;

        task_executor.merge_into_result(&mut result).await;
        Ok(result)
    }

    /// Execute a chat interaction using the agent loop.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_chat(
        &self,
        provider: &dyn ModelProvider,
        executor: &dyn AgentToolExecutor,
        project: &ProjectInfo<'_>,
        custom_system_prompt: &str,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        event_tx: Option<mpsc::Sender<AgentLoopEvent>>,
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
        event_tx: Option<&mpsc::Sender<AgentLoopEvent>>,
    ) -> Result<TaskExecutionResult, crate::AgentError> {
        let command = auto_correct_build_command(params.command)
            .unwrap_or_else(|| params.command.to_string());
        let max_attempts = self.config.max_shell_task_retries;
        let mut prior: Vec<BuildFixAttemptRecord> = Vec::new();

        for attempt in 1..=max_attempts {
            if let Some(tx) = event_tx {
                let _ = tx.try_send(AgentLoopEvent::TextDelta(format!(
                    "Running: {command} (attempt {attempt}/{max_attempts})\n",
                )));
            }

            let result = run_build_command(params.project_root, &command, None)
                .await
                .map_err(|e| crate::AgentError::BuildFailed(e.to_string()))?;

            if result.success {
                let notes = format!(
                    "Command `{}` succeeded on attempt {attempt}.\n{}",
                    command, result.stdout,
                );
                if let Some(tx) = event_tx {
                    let _ = tx.try_send(AgentLoopEvent::TextDelta(notes.clone()));
                }
                return Ok(TaskExecutionResult {
                    notes,
                    files_already_applied: false,
                    ..TaskExecutionResult::default()
                });
            }

            let detail = if result.stderr.is_empty() {
                &result.stdout
            } else {
                &result.stderr
            };

            if let Some(tx) = event_tx {
                let _ = tx.try_send(AgentLoopEvent::TextDelta(format!(
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
                    stderr: detail.clone(),
                    error_signature: normalize_error_signature(detail),
                    files_changed: Vec::new(),
                    changes_summary: String::new(),
                });
            }
        }

        Err(crate::AgentError::BuildFailed(format!(
            "command `{command}` failed after {max_attempts} attempts"
        )))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an [`AgentLoopConfig`] from task complexity and runner config.
#[must_use]
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
        TaskComplexity::Complex => config.task_execution_max_tokens.max(32_768),
        TaskComplexity::Standard => config.task_execution_max_tokens,
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
    // TODO(phase-6): wire thinking_budget into AgentLoopConfig or delete; see system-audit-refactor plan
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
        auth_token: config.auth_token.clone(),
        system_prompt,
        model,
        ..AgentLoopConfig::default()
    }
}

/// Process an [`AgentLoopResult`] into a [`TaskExecutionResult`].
fn finalize_loop_result(result: AgentLoopResult) -> TaskExecutionResult {
    let AgentLoopResult {
        total_text,
        total_input_tokens,
        total_output_tokens,
        messages,
        ..
    } = result;
    let notes = if total_text.is_empty() {
        "Task completed via agentic tool-use loop".to_string()
    } else {
        total_text
    };
    TaskExecutionResult {
        notes,
        file_ops: Vec::new(),
        follow_up_tasks: Vec::new(),
        input_tokens: total_input_tokens,
        output_tokens: total_output_tokens,
        files_already_applied: true,
        no_changes_needed: false,
        reached_implementing: false,
        messages,
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
mod tests;
