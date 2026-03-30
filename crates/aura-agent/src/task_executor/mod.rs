//! Task-aware tool executor with plan gating, file tracking, self-review,
//! and stub detection.
//!
//! [`TaskToolExecutor`] wraps an inner [`AgentToolExecutor`] to intercept
//! engine-level tools (`task_done`, `submit_plan`, `get_task_context`) and
//! enforce the explore-then-implement workflow.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};

use crate::agent_runner::FollowUpSuggestion;
use crate::build::{classify_build_errors, error_category_guidance};
use crate::events::AgentLoopEvent;
use crate::file_ops::{self, FileOp};
use crate::planning::{TaskPhase, TaskPlan};
use crate::prompts::build_stub_fix_prompt;
use crate::self_review::SelfReviewGuard;
use crate::types::{
    AgentToolExecutor, AutoBuildResult, BuildBaseline, ToolCallInfo, ToolCallResult,
};
use crate::verify::infer_default_build_command;

const MAX_STUB_FIX_ATTEMPTS: u32 = 2;

mod handlers;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// TaskToolExecutor
// ---------------------------------------------------------------------------

/// Tool executor that layers plan gating, file-op tracking, self-review,
/// and stub detection on top of a delegated executor.
pub struct TaskToolExecutor {
    /// Inner executor that handles filesystem and search tools.
    pub inner: Arc<dyn AgentToolExecutor>,
    /// Path to the project root for build and stub checks.
    pub project_folder: String,
    /// Build command (from project config or auto-detected).
    pub build_command: Option<String>,
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
    /// Current task phase (explore vs implement).
    pub task_phase: Arc<Mutex<TaskPhase>>,
    /// Self-review guard tracking writes vs reads.
    pub self_review: Arc<Mutex<SelfReviewGuard>>,
    /// Optional event channel for status messages.
    pub event_tx: Option<mpsc::UnboundedSender<AgentLoopEvent>>,
    /// Set to true when the agent explicitly declares no file changes are
    /// required for this task (via `no_changes_needed` in `task_done` input).
    pub no_changes_needed: Arc<Mutex<bool>>,
    /// Rolling counters for recent tool call outcomes (success / error).
    pub recent_tool_outcomes: Arc<Mutex<RecentToolOutcomes>>,
}

/// Tracks a rolling window of tool call success/error outcomes.
#[derive(Debug, Default)]
pub struct RecentToolOutcomes {
    pub total: usize,
    pub errors: usize,
    pub last_command_failed: bool,
}

#[async_trait]
impl AgentToolExecutor for TaskToolExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        let mut delegated_indices: Vec<usize> = Vec::new();
        let mut gated_indices: Vec<usize> = Vec::new();

        for (i, tc) in tool_calls.iter().enumerate() {
            match tc.name.as_str() {
                "task_done" | "get_task_context" | "submit_plan" => {}
                "write_file" | "edit_file" | "delete_file" => {
                    let phase = self.task_phase.lock().await;
                    if matches!(*phase, TaskPhase::Exploring) {
                        gated_indices.push(i);
                    } else {
                        self.track_file_op(&tc.name, &tc.input).await;
                        if let Some(path) = tc.input.get("path").and_then(|v| v.as_str()) {
                            self.self_review.lock().await.record_write(path);
                        }
                        delegated_indices.push(i);
                    }
                }
                _ => {
                    self.track_file_op(&tc.name, &tc.input).await;
                    if tc.name == "read_file" {
                        if let Some(path) = tc.input.get("path").and_then(|v| v.as_str()) {
                            self.self_review.lock().await.record_read(path);
                        }
                    }
                    delegated_indices.push(i);
                }
            }
        }

        // Delegate non-special tools to inner executor
        let delegated_calls: Vec<ToolCallInfo> = delegated_indices
            .iter()
            .map(|&i| tool_calls[i].clone())
            .collect();
        let delegated_results = if delegated_calls.is_empty() {
            Vec::new()
        } else {
            self.inner.execute(&delegated_calls).await
        };

        let mut delegated_iter = delegated_results.into_iter();
        let mut results = Vec::with_capacity(tool_calls.len());
        let mut stop = false;

        for (i, tc) in tool_calls.iter().enumerate() {
            if gated_indices.contains(&i) {
                results.push(ToolCallResult {
                    tool_use_id: tc.id.clone(),
                    content: "ERROR: You must call submit_plan before making file changes. \
                              Explore the codebase, form your approach, then submit your plan."
                        .to_string(),
                    is_error: true,
                    stop_loop: false,
                });
                continue;
            }
            match tc.name.as_str() {
                "task_done" => {
                    self.handle_task_done(tc, &mut results, &mut stop).await;
                }
                "get_task_context" => {
                    self.handle_get_context(tc, &mut results);
                }
                "submit_plan" => {
                    self.handle_submit_plan(tc, &mut results).await;
                }
                _ => {
                    if let Some(result) = delegated_iter.next() {
                        self.emit_tool_status(tc, &result);
                        {
                            let mut outcomes = self.recent_tool_outcomes.lock().await;
                            outcomes.total += 1;
                            if result.is_error {
                                outcomes.errors += 1;
                            }
                            if tc.name == "run_command" {
                                outcomes.last_command_failed = result.is_error;
                            }
                        }
                        results.push(result);
                    }
                }
            }
        }

        if stop {
            for r in &mut results {
                r.stop_loop = true;
            }
        }

        results
    }

    async fn auto_build_check(&self) -> Option<AutoBuildResult> {
        let project_root = Path::new(&self.project_folder);
        let cmd = self
            .build_command
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(String::from)
            .or_else(|| infer_default_build_command(project_root))?;

        self.emit_text(format!("\n[auto-build: {}]\n", cmd));

        match crate::verify::run_build_command(project_root, &cmd, None).await {
            Ok(result) => {
                let mut output = String::new();
                if !result.stdout.is_empty() {
                    output.push_str(&result.stdout);
                }
                if !result.stderr.is_empty() {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&result.stderr);
                }
                let output = if !result.success {
                    self.enrich_compiler_output(&output)
                } else {
                    output
                };
                Some(AutoBuildResult {
                    success: result.success,
                    output,
                    error_count: 0,
                })
            }
            Err(e) => {
                tracing::warn!(error = %e, "auto-build check failed to execute");
                None
            }
        }
    }

    async fn capture_build_baseline(&self) -> Option<BuildBaseline> {
        let project_root = Path::new(&self.project_folder);
        let cmd = self
            .build_command
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(String::from)
            .or_else(|| infer_default_build_command(project_root))?;

        match crate::verify::run_build_command(project_root, &cmd, None).await {
            Ok(result) if !result.success => {
                let sigs = BuildBaseline::extract_signatures(&result.stderr);
                tracing::info!(
                    count = sigs.len(),
                    "captured build baseline with pre-existing errors",
                );
                Some(BuildBaseline {
                    error_signatures: sigs,
                })
            }
            Ok(_) => Some(BuildBaseline::default()),
            Err(e) => {
                tracing::warn!(error = %e, "failed to capture build baseline");
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format a concise hint for a tool call's arguments (for status logging).
pub fn format_tool_arg_hint(tc: &ToolCallInfo) -> String {
    match tc.name.as_str() {
        "read_file" => {
            let path = tc.input.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let start = tc.input.get("start_line").and_then(|v| v.as_u64());
            let end = tc.input.get("end_line").and_then(|v| v.as_u64());
            match (start, end) {
                (Some(s), Some(e)) => format!("{path}:{s}-{e}"),
                (Some(s), None) => format!("{path}:{s}-end"),
                (None, Some(e)) => format!("{path}:1-{e}"),
                (None, None) => path.to_string(),
            }
        }
        "write_file" | "edit_file" | "delete_file" => tc
            .input
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "list_files" => tc
            .input
            .get("directory")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "search_code" => {
            let pattern = tc
                .input
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let ctx = tc.input.get("context_lines").and_then(|v| v.as_u64());
            if let Some(c) = ctx {
                format!("{pattern}, context={c}")
            } else {
                pattern.to_string()
            }
        }
        "run_command" => tc
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

/// Check if build output looks like compiler errors (Rust or TypeScript).
pub fn looks_like_compiler_errors(output: &str) -> bool {
    let has_rust_errors = output.contains("error[E") && output.contains("-->");
    let has_generic_errors = output.contains("error:") && output.contains("-->");
    let has_ts_errors = output.contains("TS2") && output.contains("error TS");
    has_rust_errors || has_generic_errors || has_ts_errors
}
