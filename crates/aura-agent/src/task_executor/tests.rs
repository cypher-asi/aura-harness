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
    let calls = [task_done_no_changes(
        "analysis task, no code changes required",
    )];
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
