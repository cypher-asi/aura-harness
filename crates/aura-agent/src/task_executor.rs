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

impl TaskToolExecutor {
    async fn track_file_op(&self, tool_name: &str, input: &serde_json::Value) {
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if path.is_empty() {
            return;
        }
        let op = match tool_name {
            "write_file" => {
                let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
                FileOp::Create {
                    path: path.to_string(),
                    content: content.to_string(),
                }
            }
            "edit_file" => FileOp::Modify {
                path: path.to_string(),
                content: String::new(),
            },
            "delete_file" => FileOp::Delete {
                path: path.to_string(),
            },
            _ => return,
        };
        self.tracked_file_ops.lock().await.push(op);
    }

    fn enrich_compiler_output(&self, raw_output: &str) -> String {
        if !looks_like_compiler_errors(raw_output) {
            return raw_output.to_string();
        }

        let base_path = Path::new(&self.project_folder);

        let categories = classify_build_errors(raw_output);
        let guidance = error_category_guidance(&categories);
        let refs = crate::verify::parse_error_references(raw_output);
        let api_ref = file_ops::resolve_error_context(base_path, &refs);

        let mut enriched = raw_output.to_string();

        if !guidance.is_empty() {
            enriched.push_str("\n\n## Error Diagnosis & Guidance\n\n");
            enriched.push_str(&guidance);
        }

        if !api_ref.is_empty() {
            enriched.push('\n');
            enriched.push_str(&api_ref);
        }

        enriched
    }

    async fn handle_task_done(
        &self,
        tc: &ToolCallInfo,
        results: &mut Vec<ToolCallResult>,
        stop: &mut bool,
    ) {
        self.extract_notes_and_follow_ups(tc).await;

        if let Some(error_prompt) = self.check_pervasive_errors().await {
            results.push(ToolCallResult {
                tool_use_id: tc.id.clone(),
                content: error_prompt,
                is_error: true,
                stop_loop: false,
            });
            return;
        }

        if let Some(review_prompt) = self.check_self_review().await {
            results.push(ToolCallResult {
                tool_use_id: tc.id.clone(),
                content: review_prompt,
                is_error: true,
                stop_loop: false,
            });
            return;
        }

        if let Some(no_write_prompt) = self.check_no_writes().await {
            results.push(ToolCallResult {
                tool_use_id: tc.id.clone(),
                content: no_write_prompt,
                is_error: true,
                stop_loop: false,
            });
            return;
        }

        if let Some(stub_prompt) = self.check_stubs_and_reject().await {
            results.push(ToolCallResult {
                tool_use_id: tc.id.clone(),
                content: stub_prompt,
                is_error: true,
                stop_loop: false,
            });
        } else {
            results.push(ToolCallResult {
                tool_use_id: tc.id.clone(),
                content: r#"{"status":"completed"}"#.to_string(),
                is_error: false,
                stop_loop: true,
            });
            *stop = true;
        }
    }

    async fn extract_notes_and_follow_ups(&self, tc: &ToolCallInfo) {
        let task_notes = tc
            .input
            .get("notes")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        {
            let mut n = self.notes.lock().await;
            *n = task_notes;
        }
        if let Some(arr) = tc.input.get("follow_ups").and_then(|v| v.as_array()) {
            let mut fu_lock = self.follow_ups.lock().await;
            for fu in arr {
                let title = fu
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let desc = fu
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                fu_lock.push(FollowUpSuggestion {
                    title,
                    description: desc,
                });
            }
        }
        if let Some(reasoning) = tc.input.get("reasoning").and_then(|v| v.as_array()) {
            let reasoning_text: Vec<String> = reasoning
                .iter()
                .filter_map(|r| r.as_str().map(String::from))
                .collect();
            if !reasoning_text.is_empty() {
                let mut n = self.notes.lock().await;
                n.push_str("\n\nReasoning:\n");
                for r in &reasoning_text {
                    n.push_str(&format!("- {r}\n"));
                }
            }
        }
        if tc
            .input
            .get("no_changes_needed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            *self.no_changes_needed.lock().await = true;
        }
    }

    async fn check_pervasive_errors(&self) -> Option<String> {
        let outcomes = self.recent_tool_outcomes.lock().await;
        if outcomes.last_command_failed {
            return Some(
                "ERROR: The last run_command failed (non-zero exit code). \
                 Your build or test is broken. Fix the errors before completing the task."
                    .to_string(),
            );
        }
        let min_calls = 6;
        let error_threshold = 0.7;
        if outcomes.total >= min_calls {
            let error_ratio = outcomes.errors as f64 / outcomes.total as f64;
            if error_ratio >= error_threshold {
                return Some(format!(
                    "ERROR: {}/{} recent tool calls returned errors ({:.0}% failure rate). \
                     The task is likely incomplete. Review the errors, fix the underlying \
                     issue, then try completing again.",
                    outcomes.errors, outcomes.total, error_ratio * 100.0,
                ));
            }
        }
        None
    }

    async fn check_self_review(&self) -> Option<String> {
        let unreviewed = self.self_review.lock().await.check_review_needed()?;
        Some(format!(
            "SELF-REVIEW REQUIRED: Before completing, re-read the files you modified \
             to verify correctness:\n{}\n\nCheck: (a) changes match task requirements, \
             (b) no placeholder/stub code remains, (c) no debug code left behind.\n\
             Then call task_done again.",
            unreviewed.join("\n"),
        ))
    }

    async fn check_no_writes(&self) -> Option<String> {
        let ops = self.tracked_file_ops.lock().await;
        if !ops.is_empty() {
            return None;
        }
        let no_changes = *self.no_changes_needed.lock().await;
        if no_changes {
            return None;
        }
        Some(
            "ERROR: You are completing this task but have not made any file changes \
             (write_file, edit_file, or delete_file). Implementation tasks must produce \
             file changes. If this task genuinely requires no file changes, call task_done \
             again with \"no_changes_needed\": true and explain why in the \"notes\" field."
                .to_string(),
        )
    }

    async fn check_stubs_and_reject(&self) -> Option<String> {
        let mut attempts = self.stub_fix_attempts.lock().await;
        if *attempts >= MAX_STUB_FIX_ATTEMPTS {
            return None;
        }
        let base_path = Path::new(&self.project_folder);
        let ops = self.tracked_file_ops.lock().await;
        let stub_reports = file_ops::detect_stub_patterns(base_path, &ops);
        if stub_reports.is_empty() {
            return None;
        }
        *attempts += 1;
        let attempt = *attempts;

        self.emit_text(format!(
            "\n[stub detection] found {} stub(s), requesting fix (attempt {}/{})\n",
            stub_reports.len(),
            attempt,
            MAX_STUB_FIX_ATTEMPTS,
        ));

        Some(build_stub_fix_prompt(&stub_reports))
    }

    async fn handle_submit_plan(&self, tc: &ToolCallInfo, results: &mut Vec<ToolCallResult>) {
        let plan = TaskPlan::from_tool_input(&tc.input);
        match plan.validate() {
            Ok(()) => {
                let context_string = plan.as_context_string();
                {
                    let mut phase = self.task_phase.lock().await;
                    *phase = TaskPhase::Implementing { plan };
                }
                results.push(ToolCallResult {
                    tool_use_id: tc.id.clone(),
                    content: format!(
                        "Plan accepted. Proceeding to implementation.\n\n\
                         YOUR PLAN (reference during implementation):\n{}\n\n\
                         Now implement according to this plan. Start with the most \
                         foundational changes first.",
                        context_string,
                    ),
                    is_error: false,
                    stop_loop: false,
                });
            }
            Err(reason) => {
                results.push(ToolCallResult {
                    tool_use_id: tc.id.clone(),
                    content: format!("Plan rejected: {reason}. Revise and resubmit."),
                    is_error: true,
                    stop_loop: false,
                });
            }
        }
    }

    fn handle_get_context(&self, tc: &ToolCallInfo, results: &mut Vec<ToolCallResult>) {
        results.push(ToolCallResult {
            tool_use_id: tc.id.clone(),
            content: self.task_context.clone(),
            is_error: false,
            stop_loop: false,
        });
    }

    fn emit_tool_status(&self, tc: &ToolCallInfo, result: &ToolCallResult) {
        let arg_hint = format_tool_arg_hint(tc);
        let status_str = if result.is_error { "error" } else { "ok" };
        let marker = if arg_hint.is_empty() {
            format!("\n[tool: {} -> {}]\n", tc.name, status_str)
        } else {
            format!("\n[tool: {}({}) -> {}]\n", tc.name, arg_hint, status_str)
        };
        self.emit_text(marker);
    }

    /// Merge tracked executor state (file ops, notes, follow-ups) into a
    /// [`TaskExecutionResult`] so that downstream consumers see real evidence
    /// instead of hardcoded defaults.
    pub async fn merge_into_result(
        &self,
        exec: &mut crate::agent_runner::TaskExecutionResult,
    ) {
        exec.file_ops = self.tracked_file_ops.lock().await.clone();
        let task_notes = self.notes.lock().await.clone();
        if !task_notes.is_empty() {
            exec.notes = task_notes;
        }
        exec.follow_up_tasks = self.follow_ups.lock().await.clone();
        exec.no_changes_needed = *self.no_changes_needed.lock().await;
        let phase = self.task_phase.lock().await;
        exec.reached_implementing = matches!(*phase, crate::planning::TaskPhase::Implementing { .. });
    }

    fn emit_text(&self, text: String) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(AgentLoopEvent::TextDelta(text));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_runner::TaskExecutionResult;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct NoOpInner;

    #[async_trait::async_trait]
    impl AgentToolExecutor for NoOpInner {
        async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
            tool_calls
                .iter()
                .map(|tc| ToolCallResult::success(&tc.id, "ok"))
                .collect()
        }
    }

    fn make_executor() -> TaskToolExecutor {
        TaskToolExecutor {
            inner: Arc::new(NoOpInner),
            project_folder: "/tmp/test".to_string(),
            build_command: None,
            task_context: String::new(),
            tracked_file_ops: Default::default(),
            notes: Default::default(),
            follow_ups: Default::default(),
            stub_fix_attempts: Default::default(),
            task_phase: Arc::new(Mutex::new(TaskPhase::Implementing {
                plan: crate::planning::TaskPlan::empty(),
            })),
            self_review: Default::default(),
            event_tx: None,
            no_changes_needed: Default::default(),
            recent_tool_outcomes: Default::default(),
        }
    }

    fn task_done_call(notes: &str) -> ToolCallInfo {
        ToolCallInfo {
            id: "td_1".to_string(),
            name: "task_done".to_string(),
            input: serde_json::json!({ "notes": notes }),
        }
    }

    fn task_done_no_changes(notes: &str) -> ToolCallInfo {
        ToolCallInfo {
            id: "td_1".to_string(),
            name: "task_done".to_string(),
            input: serde_json::json!({
                "notes": notes,
                "no_changes_needed": true,
            }),
        }
    }

    // ------------------------------------------------------------------
    // task_done guard tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn task_done_rejects_when_no_file_ops() {
        let executor = make_executor();
        let calls = [task_done_call("all done")];
        let results = executor.execute(&calls).await;

        assert_eq!(results.len(), 1);
        assert!(results[0].is_error);
        assert!(!results[0].stop_loop);
        assert!(results[0].content.contains("not made any file changes"));
    }

    #[tokio::test]
    async fn task_done_succeeds_with_file_ops() {
        let executor = make_executor();
        {
            let mut ops = executor.tracked_file_ops.lock().await;
            ops.push(FileOp::Create {
                path: "src/main.rs".to_string(),
                content: "fn main() {}".to_string(),
            });
        }
        {
            let mut sr = executor.self_review.lock().await;
            sr.record_write("src/main.rs");
            sr.record_read("src/main.rs");
        }

        let calls = [task_done_call("implemented feature")];
        let results = executor.execute(&calls).await;

        assert_eq!(results.len(), 1);
        assert!(!results[0].is_error);
        assert!(results[0].stop_loop);
        assert!(results[0].content.contains("completed"));
    }

    #[tokio::test]
    async fn task_done_allows_no_ops_with_exemption() {
        let executor = make_executor();
        let calls = [task_done_no_changes("analysis task, no code changes required")];
        let results = executor.execute(&calls).await;

        assert_eq!(results.len(), 1);
        assert!(!results[0].is_error);
        assert!(results[0].stop_loop);
        assert!(results[0].content.contains("completed"));
    }

    // ------------------------------------------------------------------
    // merge_into_result tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn merge_into_result_populates_all_fields() {
        let executor = make_executor();
        {
            let mut ops = executor.tracked_file_ops.lock().await;
            ops.push(FileOp::Create {
                path: "new.rs".to_string(),
                content: "code".to_string(),
            });
        }
        {
            let mut n = executor.notes.lock().await;
            *n = "executor notes".to_string();
        }
        {
            let mut fu = executor.follow_ups.lock().await;
            fu.push(FollowUpSuggestion {
                title: "next step".to_string(),
                description: "do more".to_string(),
            });
        }

        let mut result = TaskExecutionResult::default();
        executor.merge_into_result(&mut result).await;

        assert_eq!(result.file_ops.len(), 1);
        assert_eq!(result.notes, "executor notes");
        assert_eq!(result.follow_up_tasks.len(), 1);
        assert_eq!(result.follow_up_tasks[0].title, "next step");
        assert!(!result.no_changes_needed);
    }

    #[tokio::test]
    async fn merge_preserves_loop_notes_when_executor_notes_empty() {
        let executor = make_executor();
        let mut result = TaskExecutionResult {
            notes: "loop generated notes".to_string(),
            ..Default::default()
        };
        executor.merge_into_result(&mut result).await;

        assert_eq!(result.notes, "loop generated notes");
    }

    #[tokio::test]
    async fn merge_sets_no_changes_needed_flag() {
        let executor = make_executor();
        *executor.no_changes_needed.lock().await = true;

        let mut result = TaskExecutionResult::default();
        executor.merge_into_result(&mut result).await;

        assert!(result.no_changes_needed);
    }

    // ------------------------------------------------------------------
    // pervasive error guard tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn task_done_rejects_when_last_command_failed() {
        let executor = make_executor();
        {
            let mut ops = executor.tracked_file_ops.lock().await;
            ops.push(FileOp::Create {
                path: "src/main.rs".to_string(),
                content: "fn main() {}".to_string(),
            });
        }
        {
            let mut sr = executor.self_review.lock().await;
            sr.record_write("src/main.rs");
            sr.record_read("src/main.rs");
        }
        {
            let mut outcomes = executor.recent_tool_outcomes.lock().await;
            outcomes.total = 5;
            outcomes.errors = 1;
            outcomes.last_command_failed = true;
        }
        let calls = [task_done_call("all done")];
        let results = executor.execute(&calls).await;

        assert_eq!(results.len(), 1);
        assert!(results[0].is_error);
        assert!(!results[0].stop_loop);
        assert!(results[0].content.contains("run_command failed"));
    }

    #[tokio::test]
    async fn task_done_rejects_when_error_ratio_high() {
        let executor = make_executor();
        {
            let mut ops = executor.tracked_file_ops.lock().await;
            ops.push(FileOp::Create {
                path: "src/main.rs".to_string(),
                content: "fn main() {}".to_string(),
            });
        }
        {
            let mut sr = executor.self_review.lock().await;
            sr.record_write("src/main.rs");
            sr.record_read("src/main.rs");
        }
        {
            let mut outcomes = executor.recent_tool_outcomes.lock().await;
            outcomes.total = 10;
            outcomes.errors = 8;
            outcomes.last_command_failed = false;
        }
        let calls = [task_done_call("done")];
        let results = executor.execute(&calls).await;

        assert_eq!(results.len(), 1);
        assert!(results[0].is_error);
        assert!(results[0].content.contains("failure rate"));
    }

    #[tokio::test]
    async fn task_done_accepts_when_errors_low() {
        let executor = make_executor();
        {
            let mut ops = executor.tracked_file_ops.lock().await;
            ops.push(FileOp::Create {
                path: "src/main.rs".to_string(),
                content: "fn main() {}".to_string(),
            });
        }
        {
            let mut sr = executor.self_review.lock().await;
            sr.record_write("src/main.rs");
            sr.record_read("src/main.rs");
        }
        {
            let mut outcomes = executor.recent_tool_outcomes.lock().await;
            outcomes.total = 10;
            outcomes.errors = 2;
            outcomes.last_command_failed = false;
        }
        let calls = [task_done_call("done")];
        let results = executor.execute(&calls).await;

        assert_eq!(results.len(), 1);
        assert!(!results[0].is_error);
        assert!(results[0].stop_loop);
    }

    // ------------------------------------------------------------------
    // extract_notes_and_follow_ups tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn extract_parses_no_changes_needed_flag() {
        let executor = make_executor();
        let tc = task_done_no_changes("just an analysis");
        executor.extract_notes_and_follow_ups(&tc).await;

        assert!(*executor.no_changes_needed.lock().await);
        assert_eq!(*executor.notes.lock().await, "just an analysis");
    }
}
