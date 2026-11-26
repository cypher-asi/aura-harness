//! High-level agent execution: agentic task and chat runners.
//!
//! `AgentRunner` combines task context setup, agent loop configuration, and
//! result processing into a convenient orchestration layer built on top of
//! [`AgentLoop`].

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

use aura_model::{Message, ModelProvider, ToolDefinition};

use crate::agent_loop::{AgentLoop, AgentLoopConfig};
use crate::events::AgentLoopEvent;
use crate::planning::{TaskPhase, TaskPlan};
use aura_context_prompts::{
    agentic_execution_system_prompt, build_chat_system_prompt, AgentInfo, ProjectInfo, SessionInfo,
    SpecInfo, TaskInfo,
};

use crate::task_context;
use crate::task_executor::TaskToolExecutor;
use crate::turn_config::classify_task_complexity;
use crate::types::{AgentLoopResult, AgentToolExecutor};

mod config;
mod loop_setup;

pub use config::AgentRunnerConfig;
pub use loop_setup::configure_loop_config;
use loop_setup::{
    build_full_task_ctx, early_test_oracle_config, ensure_loop_completed, finalize_loop_result,
};

// ---------------------------------------------------------------------------
// Internal call-shape helpers
// ---------------------------------------------------------------------------

/// Phase 8 bundle of the per-call options that distinguish the
/// `execute_task` and `execute_task_tracked` entry points when both
/// funnel through [`AgentRunner::execute_task_inner`]. Carries the
/// optional phase-reset signal (tracked path), the optional
/// pre-built task context bundle (tracked path), and the per-call
/// `early_test_oracle` toggle (either runner default or per-task
/// override). Bundling keeps `execute_task_inner` under the
/// `too-many-arguments` ceiling without flattening the optional
/// fields into the public API.
struct TaskInnerOptions {
    require_task_done: bool,
    phase_reset_signal: Option<Arc<AtomicBool>>,
    prebuilt_task_ctx: Option<String>,
    early_test_oracle: bool,
}

/// Phase 8 bundle of the prompt-shaping inputs for
/// [`AgentRunner::execute_chat`]. Both fields feed the chat-system
/// prompt builder; grouping them keeps the public `execute_chat`
/// signature under the clippy ceiling without losing the per-caller
/// flexibility (the wider call sites still pass an owned
/// [`ProjectInfo`] and a `&str` prompt body).
pub struct ChatPromptCtx<'a> {
    pub project: &'a ProjectInfo<'a>,
    pub custom_system_prompt: &'a str,
}

/// Phase 8 bundle of the optional coordination handles for
/// [`AgentRunner::execute_chat`]. Pairs the streaming
/// [`AgentLoopEvent`] sink with the external cancellation signal so
/// the public `execute_chat` signature stays under the
/// `clippy::too_many_arguments` ceiling without forcing every
/// caller to thread two trailing `Option<…>` arguments.
#[derive(Default)]
pub struct ChatHooks {
    pub event_tx: Option<mpsc::Sender<AgentLoopEvent>>,
    pub cancel: Option<CancellationToken>,
}

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// Result of executing an agentic task.
///
/// Phase 7 dropped the `no_changes_needed`, `file_ops`, and
/// `reached_implementing` fields: the automaton finalizer
/// (`builtins::common::finalize::finalize_task_outcome`) only
/// consumes `notes` + `input_tokens` + `output_tokens` on the
/// success path, and the corresponding `CommitSkipped` /
/// `FileOpsApplied` events that would have consumed the dropped
/// fields had no emitters or external consumers in this workspace.
/// The internal `TaskToolExecutor.tracked_file_ops` /
/// `no_changes_needed` / `task_phase` state still drives the
/// `task_done` rejection semantics inside the loop — that surface
/// is independent of this struct.
#[derive(Debug, Clone, Default)]
pub struct TaskExecutionResult {
    pub notes: String,
    pub follow_up_tasks: Vec<FollowUpSuggestion>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub files_already_applied: bool,
    /// Final message history from the agent loop. Downstream validators use
    /// this to build recovery hints (e.g. which file paths the agent tried
    /// to write before truncation).
    pub messages: Vec<aura_model::Message>,
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

/// Parameters for running an agentic task.
pub struct AgenticTaskParams<'a> {
    pub project: &'a ProjectInfo<'a>,
    pub spec: &'a SpecInfo<'a>,
    pub task: &'a TaskInfo<'a>,
    pub session: &'a SessionInfo<'a>,
    pub work_log: &'a [String],
    pub completed_deps: &'a [TaskInfo<'a>],
    pub workspace_map: &'a str,
    pub codebase_snapshot: &'a str,
    pub type_defs_context: &'a str,
    pub dep_api_context: &'a str,
    pub member_count: usize,
    pub tools: Vec<ToolDefinition>,
    /// 0-indexed retry counter. `0` is the first attempt (a fresh task
    /// or an initial run after a process restart), `1+` is a retry.
    /// Gates Phase 4 context enrichment: pre-resolved paths/symbols are
    /// only spliced into the initial user message on attempt 0, since
    /// retries get a different, narrower context from Phase 5's
    /// decomposition path (and re-injecting the same hints wastes
    /// tokens without adding signal).
    pub attempt: u32,
    /// Agent identity / skills / operator-authored system prompt
    /// threaded from the wire layer (PR B re-adds the field that PR A
    /// removed alongside the dead `build_agent_preamble`).
    ///
    /// PR B: every construction site passes `None`. The
    /// `agentic_execution_system_prompt` builder is wired to consume
    /// the bundle so PR C's aura-os populator can flip a single field
    /// and have identity flow into the model-facing prompt.
    pub agent: Option<&'a AgentInfo<'a>>,
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
    /// Test command used by the `task_done` hard gate. Forwarded directly
    /// onto [`crate::task_executor::TaskToolExecutor::test_command`].
    pub test_command: Option<String>,
    /// Phase 5: per-task switch for the early test-gate oracle.
    /// `None` falls back to [`AgentRunnerConfig::early_test_oracle`];
    /// `Some(v)` overrides it for this one task. The `TaskRunAutomaton`
    /// populates this from `TaskRunConfig.early_test_oracle` parsed
    /// off the dispatch JSON so per-task opt-outs (e.g. ad-hoc chat-shaped
    /// runs) are honored without rebuilding the shared
    /// [`AgentRunner`].
    pub early_test_oracle: Option<bool>,
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
        // Non-tracked callers (e.g. chat) do not share an exploration-
        // reset signal with a `TaskToolExecutor`; pass `None` so the
        // loop simply does not perform the reset.
        self.execute_task_inner(
            provider,
            executor,
            params,
            event_tx,
            cancel,
            TaskInnerOptions {
                phase_reset_signal: None,
                prebuilt_task_ctx: None,
                require_task_done: false,
                early_test_oracle: self.config.early_test_oracle,
            },
        )
        .await
    }

    async fn execute_task_inner(
        &self,
        provider: &dyn ModelProvider,
        executor: &dyn AgentToolExecutor,
        params: &AgenticTaskParams<'_>,
        event_tx: Option<mpsc::Sender<AgentLoopEvent>>,
        cancel: Option<CancellationToken>,
        options: TaskInnerOptions,
    ) -> Result<TaskExecutionResult, crate::AgentError> {
        let TaskInnerOptions {
            require_task_done,
            phase_reset_signal,
            prebuilt_task_ctx,
            early_test_oracle,
        } = options;
        let complexity = classify_task_complexity(params.task.title, params.task.description);

        let test_command_override = aura_config::agent().verify.test_command_override.clone();
        let system_prompt = agentic_execution_system_prompt(
            params.project,
            params.agent,
            test_command_override.as_deref(),
        );

        // Reuse the caller-supplied full task context bundle when
        // present (the `execute_task_tracked` path pre-builds it so
        // the `get_task_context` tool can return a non-empty payload
        // — see `build_full_task_ctx`) and only build a fresh one for
        // the non-tracked path. Either way we cap the copy used as
        // the initial user message via `cap_bootstrap_task_context`
        // so model routing stays small; the full bundle (when
        // provided) is the executor's source of truth for the tool
        // response and stays untruncated.
        let mut task_ctx = match prebuilt_task_ctx {
            Some(ctx) => ctx,
            None => build_full_task_ctx(params).await,
        };
        task_context::cap_bootstrap_task_context(&mut task_ctx);

        let mut loop_config =
            configure_loop_config(complexity, &self.config, params.member_count, system_prompt);
        loop_config.phase_reset_signal = phase_reset_signal;
        // Phase 5: install the EarlyTestOracle source via the
        // per-call `early_test_oracle` flag (the runner-level
        // default from `AgentRunnerConfig.early_test_oracle` for
        // `execute_task` callers, or the per-task override from
        // `TaskTrackingConfig.early_test_oracle` for
        // `execute_task_tracked` callers). The oracle is permanently
        // disarmed when the project declares no `test_command`, so
        // we only attach the config when both `enabled` AND a
        // non-blank command are present.
        loop_config.early_test_oracle = early_test_oracle_config(early_test_oracle, params);

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
            .await?;

        ensure_loop_completed(&result, require_task_done)?;
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
        // Shared `Arc<AtomicBool>` between the task executor and the
        // wrapped agent loop. Pre-seeded to `true` so the first
        // iteration's `LoopState::begin_iteration` zeroes exploration
        // and read-guard counters, bumps the allowance with the
        // implement-phase bonus, and arms the post-plan exploration
        // hard block — giving every task the fresh budget that used to
        // require a successful `submit_plan` call.
        // `handle_submit_plan` still flips this back to `true` when a
        // plan is accepted mid-run so an explicit re-plan also gets a
        // fresh budget. See `harness_task_completion_fix` Phase 1.
        let reset_signal = Arc::new(AtomicBool::new(true));
        // Build the full (uncapped) task-context bundle ONCE here so
        // both the `get_task_context` tool response and the initial
        // user message inside `execute_task_inner` derive from the
        // same source. Historically `task_context` was hard-coded to
        // `String::new()` on the executor, so every call to
        // `get_task_context` returned an empty payload and forced the
        // agent into a long `list_files`/`read_file`/`search_code`
        // burn just to rediscover the task description the server
        // already had — see the `submit_plan` deadlock investigation.
        // Phase 4 update: the helper is now async because attempt-0
        // contexts splice in a pre-resolved paths/symbols block (best
        // effort, 2s per-call timeout, falls back cleanly to the
        // pre-Phase-4 prompt on any IO failure). Still called ONCE so
        // both the `get_task_context` tool response and the initial
        // user message inside `execute_task_inner` derive from the
        // same source, and so we never pay the resolve cost twice.
        let full_task_ctx = build_full_task_ctx(params).await;
        // Phase 5: per-task override on top of the runner-level
        // default. Captured before the `tracking` struct is moved
        // into the executor below.
        let early_test_oracle = tracking
            .early_test_oracle
            .unwrap_or(self.config.early_test_oracle);
        let task_executor = TaskToolExecutor {
            inner: tracking.inner_executor,
            project_folder: tracking.project_folder,
            build_command: tracking.build_command,
            test_command: tracking.test_command,
            test_command_override: crate::task_executor::read_test_command_override_env(),
            task_context: full_task_ctx.clone(),
            tracked_file_ops: Arc::default(),
            notes: Arc::default(),
            follow_ups: Arc::default(),
            stub_fix_attempts: Arc::default(),
            test_runner: Arc::new(crate::task_executor::RealTaskTestRunner),
            task_phase: Arc::new(Mutex::new(TaskPhase::Implementing {
                plan: TaskPlan::empty(),
            })),
            self_review: Arc::default(),
            event_tx: event_tx.clone(),
            no_changes_needed: Arc::default(),
            recent_tool_outcomes: Arc::default(),
            reset_explore_on_phase_change: Arc::clone(&reset_signal),
        };

        let mut result = self
            .execute_task_inner(
                provider,
                &task_executor,
                params,
                event_tx,
                cancel,
                TaskInnerOptions {
                    require_task_done: true,
                    phase_reset_signal: Some(reset_signal),
                    prebuilt_task_ctx: Some(full_task_ctx),
                    early_test_oracle,
                },
            )
            .await?;

        task_executor.merge_into_result(&mut result).await;
        Ok(result)
    }

    /// Execute a chat interaction using the agent loop.
    pub async fn execute_chat(
        &self,
        provider: &dyn ModelProvider,
        executor: &dyn AgentToolExecutor,
        prompt: ChatPromptCtx<'_>,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        hooks: ChatHooks,
    ) -> Result<AgentLoopResult, crate::AgentError> {
        let ChatHooks { event_tx, cancel } = hooks;
        let ChatPromptCtx {
            project,
            custom_system_prompt,
        } = prompt;
        let system_prompt = {
            let project_id = project.project_id.map(str::to_owned);
            let name = project.name.to_owned();
            let description = project.description.to_owned();
            let folder_path = project.folder_path.to_owned();
            let build_command = project.build_command.map(str::to_owned);
            let test_command = project.test_command.map(str::to_owned);
            let custom = custom_system_prompt.to_owned();
            tokio::task::spawn_blocking(move || {
                let p = ProjectInfo {
                    project_id: project_id.as_deref(),
                    name: &name,
                    description: &description,
                    folder_path: &folder_path,
                    build_command: build_command.as_deref(),
                    test_command: test_command.as_deref(),
                };
                build_chat_system_prompt(&p, &custom, None)
            })
            .await?
        };
        let config = AgentLoopConfig {
            system_prompt,
            stream_timeout: Duration::from_secs(self.config.stream_timeout_secs),
            billing_reason: "aura_chat".to_string(),
            max_context_tokens: Some(self.config.max_context_tokens),
            // Mirror `configure_loop_config`: when an automaton's
            // built-in chat (e.g. the dev-loop's planner chat) goes
            // through this path, propagate the same router/billing
            // identifiers so the outbound headers match the
            // interactive WebSocket chat path.
            aura_org_id: self.config.aura_org_id.clone(),
            aura_session_id: self.config.aura_session_id.clone(),
            aura_agent_id: self.config.aura_agent_id.clone(),
            aura_project_id: self.config.aura_project_id.clone(),
            prompt_cache_key: self.config.prompt_cache_key.clone(),
            ..AgentLoopConfig::for_agent(self.config.default_model.clone())
        };
        let agent_loop = AgentLoop::new(config);
        agent_loop
            .run_with_events(provider, executor, messages, tools, event_tx, cancel)
            .await
    }
}

#[cfg(test)]
mod tests;
