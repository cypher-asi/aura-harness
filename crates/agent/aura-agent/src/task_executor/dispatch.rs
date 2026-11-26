//! Steps of [`crate::types::AgentToolExecutor::execute`] for
//! [`TaskToolExecutor`]: pre-dispatch tracking/blocking, delegation to the
//! inner executor, and result assembly.

use std::collections::HashMap;

use super::outcomes::allowed_after_no_write_task_done_reject;
use super::TaskToolExecutor;
use crate::types::{ToolCallInfo, ToolCallResult};

/// Output of the pre-dispatch pass over a batch of tool calls.
pub(super) struct DispatchPlan {
    /// Indices of calls forwarded to the inner executor.
    pub(super) delegated_indices: Vec<usize>,
    /// Calls blocked by a task guard, keyed by index.
    pub(super) blocked_results: HashMap<usize, ToolCallResult>,
}

fn is_engine_tool(name: &str) -> bool {
    matches!(name, "task_done" | "get_task_context" | "submit_plan")
}

fn no_writes_after_reject_result(tc: &ToolCallInfo) -> ToolCallResult {
    ToolCallResult {
        tool_use_id: tc.id.clone(),
        content: aura_context_prompts::model_messages::task_done::NO_WRITES_AFTER_REJECT_BODY
            .to_string(),
        is_error: true,
        kind: aura_core_types::ToolResultKind::AgentError,
        stop_loop: false,
        file_changes: Vec::new(),
        image: None,
    }
}

impl TaskToolExecutor {
    /// Track every call and decide which ones are blocked or delegated.
    pub(super) async fn plan_dispatch(&self, tool_calls: &[ToolCallInfo]) -> DispatchPlan {
        let mut plan = DispatchPlan {
            delegated_indices: Vec::new(),
            blocked_results: HashMap::new(),
        };
        let block_exploration_after_no_write_reject = self
            .recent_tool_outcomes
            .lock()
            .await
            .task_done_no_writes_rejected;

        for (i, tc) in tool_calls.iter().enumerate() {
            if block_exploration_after_no_write_reject
                && !allowed_after_no_write_task_done_reject(&tc.name)
            {
                plan.blocked_results
                    .insert(i, no_writes_after_reject_result(tc));
                continue;
            }
            if self.track_call(tc).await {
                plan.delegated_indices.push(i);
            }
        }
        plan
    }

    /// Record file-op / self-review tracking for one call. Returns whether
    /// the call goes to the inner executor.
    async fn track_call(&self, tc: &ToolCallInfo) -> bool {
        match tc.name.as_str() {
            "task_done" | "get_task_context" | "submit_plan" => false,
            "write_file" | "edit_file" | "delete_file" => {
                self.recent_tool_outcomes
                    .lock()
                    .await
                    .task_done_no_writes_rejected = false;
                self.track_file_op(&tc.name, &tc.input).await;
                if let Some(path) = tc.input.get("path").and_then(|v| v.as_str()) {
                    self.self_review.lock().await.record_write(path);
                }
                true
            }
            _ => {
                self.track_file_op(&tc.name, &tc.input).await;
                if tc.name == "read_file" {
                    if let Some(path) = tc.input.get("path").and_then(|v| v.as_str()) {
                        self.self_review.lock().await.record_read(path);
                    }
                }
                true
            }
        }
    }

    /// Delegate non-special tools to the inner executor.
    pub(super) async fn run_delegated(
        &self,
        tool_calls: &[ToolCallInfo],
        delegated_indices: &[usize],
    ) -> Vec<ToolCallResult> {
        let delegated_calls: Vec<ToolCallInfo> = delegated_indices
            .iter()
            .map(|&i| tool_calls[i].clone())
            .collect();
        if delegated_calls.is_empty() {
            Vec::new()
        } else {
            self.inner.execute(&delegated_calls).await
        }
    }

    /// Assemble results in call order. Returns the results and whether an
    /// accepted `task_done` asked the loop to stop.
    pub(super) async fn collect_results(
        &self,
        tool_calls: &[ToolCallInfo],
        mut blocked_results: HashMap<usize, ToolCallResult>,
        delegated_results: Vec<ToolCallResult>,
    ) -> (Vec<ToolCallResult>, bool) {
        let mut delegated_iter = delegated_results.into_iter();
        let mut results = Vec::with_capacity(tool_calls.len());
        let mut stop = false;

        for (i, tc) in tool_calls.iter().enumerate() {
            if let Some(result) = blocked_results.remove(&i) {
                results.push(
                    self.inner
                        .spawn_preset(tc.clone(), "task_guard", result)
                        .await,
                );
                continue;
            }
            if is_engine_tool(&tc.name) {
                let run = Box::pin(self.handle_engine_tool(tc));
                let result = self
                    .inner
                    .spawn_local(tc.clone(), "task_executor", run)
                    .await;
                stop |= tc.name == "task_done" && result.stop_loop;
                results.push(result);
            } else if let Some(result) = delegated_iter.next() {
                self.emit_tool_status(tc, &result);
                {
                    let mut outcomes = self.recent_tool_outcomes.lock().await;
                    outcomes.record(&tc.name, result.is_error, &result.content);
                }
                results.push(result);
            }
        }
        (results, stop)
    }
}
