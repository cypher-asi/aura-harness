//! Task-aware tool executor with plan gating, file tracking, self-review,
//! and stub detection.
//!
//! [`TaskToolExecutor`] wraps an inner [`AgentToolExecutor`] to intercept
//! engine-level tools (`task_done`, `submit_plan`, `get_task_context`) and
//! enforce the explore-then-implement workflow. Engine tools and guard
//! blocks still go through the inner executor (`spawn_local` /
//! `spawn_preset`), so a kernel-backed inner records and gates them.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};

use crate::agent_runner::FollowUpSuggestion;
use crate::build::{classify_build_errors, error_category_guidance};
use crate::events::AgentLoopEvent;
use crate::file_ops::{self, FileOp};
use crate::planning::{TaskPhase, TaskPlan};
use crate::self_review::SelfReviewGuard;
use crate::types::{
    AgentToolExecutor, AutoBuildResult, BuildBaseline, ToolCallInfo, ToolCallResult,
};
use crate::verify::runner::BuildRunnerError;
use crate::verify::{infer_default_test_command, TestSuiteOutcome};

pub(crate) use aura_config::MAX_STUB_FIX_ATTEMPTS;

/// Read the operator override for the project test command from
/// `aura_config::agent().verify.test_command_override` (sourced from
/// `AURA_DOD_TEST_COMMAND` at startup).
///
/// This remains an operator override, not a fallback. Empty or
/// whitespace-only values are treated as unset by the config layer.
///
/// Resolution order at call time, highest precedence first:
///   1. `aura_config::agent().verify.test_command_override`
///      (captured from `AURA_DOD_TEST_COMMAND` at startup)
///   2. `Project.test_command` (per-project config)
///   3. `infer_default_test_command(project_root)` (manifest auto-detect)
#[must_use]
pub(crate) fn read_test_command_override_env() -> Option<String> {
    aura_config::agent().verify.test_command_override.clone()
}

/// Pluggable test runner used by the `task_done` hard gate.
///
/// Real automatons get [`RealTaskTestRunner`], which shells out to
/// [`crate::verify::run_full_test_suite`]. Unit tests inject a deterministic
/// mock so the gate can be exercised without spinning up a child process.
#[async_trait]
pub(crate) trait TaskTestRunner: Send + Sync + std::fmt::Debug {
    async fn run_tests(
        &self,
        project_root: &Path,
        command: &str,
    ) -> Result<TestSuiteOutcome, BuildRunnerError>;
}

/// Default [`TaskTestRunner`] implementation that runs the project's test
/// command via the verify module.
#[derive(Debug, Default)]
pub(crate) struct RealTaskTestRunner;

#[async_trait]
impl TaskTestRunner for RealTaskTestRunner {
    async fn run_tests(
        &self,
        project_root: &Path,
        command: &str,
    ) -> Result<TestSuiteOutcome, BuildRunnerError> {
        crate::verify::run_full_test_suite(project_root, command).await
    }
}

mod arg_hint;
mod build_check;
mod dispatch;
mod handlers;
mod outcomes;

pub(crate) use arg_hint::format_tool_arg_hint;
pub(crate) use build_check::looks_like_compiler_errors;
pub use build_check::run_project_build_check;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// TaskToolExecutor
// ---------------------------------------------------------------------------

/// Tool executor that layers plan gating, file-op tracking, self-review,
/// and stub detection on top of a delegated executor.
pub(crate) struct TaskToolExecutor {
    /// Inner executor that handles filesystem and search tools.
    pub inner: Arc<dyn AgentToolExecutor>,
    /// Path to the project root for build and stub checks.
    pub project_folder: String,
    /// Build command (from project config or auto-detected).
    pub build_command: Option<String>,
    /// Test command used by the best-effort post-`task_done` test run.
    /// When `None`, the executor tries to infer one from the project
    /// manifest and otherwise no-ops. Configured per project via
    /// `Project.test_command`. Codex parity: failures are a warning,
    /// not a gate.
    pub test_command: Option<String>,
    /// Operator override for the project test command, captured at executor
    /// construction from `aura_config::agent().verify.test_command_override`
    /// (sourced once from `AURA_DOD_TEST_COMMAND` at startup). When `Some`,
    /// this wins over [`Self::test_command`] and any inferred default.
    /// Captured at construction so concurrent tests that install a custom
    /// `aura_config` don't race the executor.
    pub test_command_override: Option<String>,
    /// Pre-built task context for `get_task_context` handler.
    pub task_context: String,
    /// Tracked file operations for stub detection.
    pub tracked_file_ops: Arc<Mutex<Vec<FileOp>>>,
    /// Completion notes accumulated by `task_done`.
    pub notes: Arc<Mutex<String>>,
    /// Follow-up suggestions from `task_done`.
    pub follow_ups: Arc<Mutex<Vec<FollowUpSuggestion>>>,
    /// Counter for stub-fix rejection attempts.
    pub stub_fix_attempts: Arc<Mutex<u32>>,
    /// Pluggable runner the post-`task_done` best-effort test run
    /// uses to execute the project's test suite. Tests inject a mock;
    /// real automatons use [`RealTaskTestRunner`].
    pub test_runner: Arc<dyn TaskTestRunner>,
    /// Current task phase (explore vs implement).
    pub task_phase: Arc<Mutex<TaskPhase>>,
    /// Self-review guard tracking writes vs reads.
    pub self_review: Arc<Mutex<SelfReviewGuard>>,
    /// Optional event channel for status messages.
    pub event_tx: Option<mpsc::Sender<AgentLoopEvent>>,
    /// Set to true when the agent explicitly declares no file changes are
    /// required for this task (via `no_changes_needed` in `task_done` input).
    pub no_changes_needed: Arc<Mutex<bool>>,
    /// Rolling counters for recent tool call outcomes (success / error).
    pub recent_tool_outcomes: Arc<Mutex<RecentToolOutcomes>>,
    /// Shared flag observed by the agent loop's
    /// [`crate::agent_loop::AgentLoopConfig::phase_reset_signal`]: when
    /// the loop sees it set, [`crate::agent_loop::LoopState::begin_iteration`]
    /// zeroes the exploration/read-guard counters, bumps the allowance
    /// with the implement-phase bonus, and arms the post-plan
    /// exploration hard block.
    ///
    /// As of harness-v2 the production `execute_task_tracked` constructor
    /// pre-seeds this to `true` so the very first iteration of every task
    /// gets the fresh-budget reset. `handle_submit_plan` still flips it
    /// back to `true` on an accepted plan so an explicit mid-run plan
    /// also resets the budget. Defaults to `false` when the executor is
    /// built standalone (e.g. unit tests) so legacy fixtures keep their
    /// pre-v2 semantics.
    pub reset_explore_on_phase_change: Arc<AtomicBool>,
}

/// Capacity of the rolling outcome window. Sized to comfortably cover a
/// single implementation burst (submit_plan + ~10-15 file/search ops +
/// a handful of retries) without letting errors from earlier in the
/// turn veto a `task_done` that the agent has clearly recovered from.
pub(crate) use aura_config::RECENT_OUTCOMES_WINDOW;

pub(crate) use outcomes::{is_policy_denial, RecentToolOutcomes};

#[async_trait]
impl AgentToolExecutor for TaskToolExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        let plan = self.plan_dispatch(tool_calls).await;
        let delegated_results = self
            .run_delegated(tool_calls, &plan.delegated_indices)
            .await;
        let (mut results, stop) = self
            .collect_results(tool_calls, plan.blocked_results, delegated_results)
            .await;

        if stop {
            for r in &mut results {
                r.stop_loop = true;
            }
        }

        results
    }

    fn spawn_preset<'a>(
        &'a self,
        call: ToolCallInfo,
        source: &'static str,
        result: ToolCallResult,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolCallResult> + Send + 'a>> {
        self.inner.spawn_preset(call, source, result)
    }

    fn spawn_local<'a>(
        &'a self,
        call: ToolCallInfo,
        source: &'static str,
        run: std::pin::Pin<Box<dyn std::future::Future<Output = ToolCallResult> + Send + 'a>>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolCallResult> + Send + 'a>> {
        self.inner.spawn_local(call, source, run)
    }

    async fn auto_build_check(&self) -> Option<AutoBuildResult> {
        self.run_auto_build_check().await
    }

    async fn capture_build_baseline(&self) -> Option<BuildBaseline> {
        // Dev-loop tasks use `TaskToolExecutor`. Skip baseline capture so
        // auto-build feedback is full stderr (Codex-style) without
        // "ignore PRE-EXISTING" annotations that steer agents away from
        // whole-tree compile-fix work.
        None
    }
}

impl TaskToolExecutor {
    /// Run an engine tool this executor answers itself. Only called once
    /// the kernel has recorded and allowed the call.
    async fn handle_engine_tool(&self, tc: &ToolCallInfo) -> ToolCallResult {
        match tc.name.as_str() {
            "task_done" => self.handle_task_done(tc).await,
            "submit_plan" => self.handle_submit_plan(tc).await,
            _ => self.handle_get_context(tc),
        }
    }

    /// Gate `task_done` when a build command is configured and the tree is red.
    pub(super) async fn check_build_before_completion(&self) -> Option<String> {
        run_project_build_check(&self.project_folder, self.build_command.as_deref())
            .await
            .err()
    }
}
