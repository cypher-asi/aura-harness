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
        for _ in 0..4 {
            outcomes.record("read_file", false, "ok");
        }
        outcomes.record("run_command", true, "exit code 1\nerror: test failed");
    }
    let calls = [task_done_call("all done")];
    let results = executor.execute(&calls).await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(!results[0].stop_loop);
    assert!(results[0].content.contains("run_command"));
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
        for _ in 0..2 {
            outcomes.record("read_file", false, "ok");
        }
        for _ in 0..8 {
            outcomes.record("read_file", true, "file not found");
        }
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
        for _ in 0..8 {
            outcomes.record("read_file", false, "ok");
        }
        for _ in 0..2 {
            outcomes.record("read_file", true, "file not found");
        }
    }
    let calls = [task_done_call("done")];
    let results = executor.execute(&calls).await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].is_error);
    assert!(results[0].stop_loop);
}

#[tokio::test]
async fn task_done_ignores_policy_denied_run_command() {
    // Task 2.4 regression: agent called `run_command` which was denied
    // by policy. `last_command_failed` must NOT be set because nothing
    // ran — there is no broken build to fix.
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
        outcomes.record("run_command", true, "Tool 'run_command' is not allowed");
    }
    let calls = [task_done_call("all done")];
    let results = executor.execute(&calls).await;

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].is_error,
        "task_done should accept: {}",
        results[0].content
    );
    assert!(results[0].stop_loop);
}

#[tokio::test]
async fn policy_denials_do_not_count_against_error_ratio() {
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
        // 9 policy denials + 1 real success -> ratio should be 0/10,
        // not 9/10. Without policy classification this would reject.
        for _ in 0..9 {
            outcomes.record("run_command", true, "Tool 'run_command' is not allowed");
        }
        outcomes.record("read_file", false, "ok");
    }
    let calls = [task_done_call("done")];
    let results = executor.execute(&calls).await;

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].is_error,
        "should not be blocked by policy denials: {}",
        results[0].content
    );
}

#[tokio::test]
async fn submit_plan_resets_outcome_window() {
    let executor = TaskToolExecutor {
        inner: Arc::new(NoOpInner),
        project_folder: "/tmp/test".to_string(),
        build_command: None,
        task_context: String::new(),
        tracked_file_ops: Default::default(),
        notes: Default::default(),
        follow_ups: Default::default(),
        stub_fix_attempts: Default::default(),
        task_phase: Arc::new(Mutex::new(TaskPhase::Exploring)),
        self_review: Default::default(),
        event_tx: None,
        no_changes_needed: Default::default(),
        recent_tool_outcomes: Default::default(),
    };
    // Simulate a noisy exploration phase: 10 errors accumulated.
    {
        let mut outcomes = executor.recent_tool_outcomes.lock().await;
        for _ in 0..10 {
            outcomes.record("read_file", true, "is not a file");
        }
        assert_eq!(outcomes.total(), 10);
    }

    // Submit a valid plan.
    let plan_call = ToolCallInfo {
        id: "sp_1".into(),
        name: "submit_plan".into(),
        input: serde_json::json!({
            "approach": "fix the bug by adding a null check that prevents the crash",
            "files_to_modify": ["src/main.rs"],
            "key_decisions": ["use an early return"],
        }),
    };
    let _ = executor.execute(&[plan_call]).await;

    // Outcome window must be cleared so the implementing phase starts
    // fresh.
    let outcomes = executor.recent_tool_outcomes.lock().await;
    assert_eq!(outcomes.total(), 0);
    assert_eq!(outcomes.real_errors(), 0);
    assert!(!outcomes.last_command_failed);
}

#[tokio::test]
async fn outcome_window_is_bounded() {
    let mut outcomes = RecentToolOutcomes::default();
    for _ in 0..100 {
        outcomes.record("read_file", true, "fail");
    }
    assert!(outcomes.total() <= RECENT_OUTCOMES_WINDOW);
    assert_eq!(outcomes.total(), RECENT_OUTCOMES_WINDOW);
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
