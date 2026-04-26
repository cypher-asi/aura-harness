use super::*;
use crate::agent_runner::TaskExecutionResult;
use crate::verify::TestSuiteOutcome;
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

/// Test [`TaskTestRunner`] that returns a queue of pre-canned outcomes.
///
/// Each call pops the next outcome; the queue is intentionally finite so a
/// runaway gate-loop in tests fails loudly with a panic instead of silently
/// reusing the last outcome.
#[derive(Debug, Default)]
struct MockTestRunner {
    queue: Mutex<Vec<anyhow::Result<TestSuiteOutcome>>>,
    calls: Mutex<u32>,
}

impl MockTestRunner {
    fn always_pass() -> Self {
        let mut q = Vec::new();
        for _ in 0..16 {
            q.push(Ok(TestSuiteOutcome {
                passed: true,
                summary: "10 passed, 0 failed".to_string(),
                ..Default::default()
            }));
        }
        Self {
            queue: Mutex::new(q),
            calls: Mutex::new(0),
        }
    }

    fn always_fail() -> Self {
        let mut q = Vec::new();
        for _ in 0..(MAX_TASK_DONE_TEST_RETRIES + 4) {
            q.push(Ok(TestSuiteOutcome {
                passed: false,
                summary: "9 passed, 1 failed".to_string(),
                failed_tests: vec!["my_crate::tests::it_works".to_string()],
                raw_stderr: "thread 'it_works' panicked at 'assertion failed'".to_string(),
                ..Default::default()
            }));
        }
        Self {
            queue: Mutex::new(q),
            calls: Mutex::new(0),
        }
    }
}

#[async_trait::async_trait]
impl TaskTestRunner for MockTestRunner {
    async fn run_tests(
        &self,
        _project_root: &std::path::Path,
        _command: &str,
    ) -> anyhow::Result<TestSuiteOutcome> {
        *self.calls.lock().await += 1;
        let mut q = self.queue.lock().await;
        if q.is_empty() {
            anyhow::bail!("MockTestRunner queue exhausted");
        }
        q.remove(0)
    }
}

fn make_executor_with_runner(runner: Arc<dyn TaskTestRunner>) -> TaskToolExecutor {
    TaskToolExecutor {
        inner: Arc::new(NoOpInner),
        project_folder: "/tmp/test".to_string(),
        build_command: None,
        test_command: Some("cargo test --workspace".to_string()),
        task_context: String::new(),
        tracked_file_ops: Default::default(),
        notes: Default::default(),
        follow_ups: Default::default(),
        stub_fix_attempts: Default::default(),
        test_gate_attempts: Default::default(),
        test_runner: runner,
        disable_test_gate: false,
        task_phase: Arc::new(Mutex::new(TaskPhase::Implementing {
            plan: crate::planning::TaskPlan::empty(),
        })),
        self_review: Default::default(),
        event_tx: None,
        no_changes_needed: Default::default(),
        dod_test_gate_exhausted: Default::default(),
        recent_tool_outcomes: Default::default(),
    }
}

fn make_executor() -> TaskToolExecutor {
    make_executor_with_runner(Arc::new(MockTestRunner::always_pass()))
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
        test_command: Some("cargo test --workspace".to_string()),
        task_context: String::new(),
        tracked_file_ops: Default::default(),
        notes: Default::default(),
        follow_ups: Default::default(),
        stub_fix_attempts: Default::default(),
        test_gate_attempts: Default::default(),
        test_runner: Arc::new(MockTestRunner::always_pass()),
        disable_test_gate: false,
        task_phase: Arc::new(Mutex::new(TaskPhase::Exploring)),
        self_review: Default::default(),
        event_tx: None,
        no_changes_needed: Default::default(),
        dod_test_gate_exhausted: Default::default(),
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

// ------------------------------------------------------------------
// task_done test gate (Definition-of-Done) tests
// ------------------------------------------------------------------

async fn seed_with_file_op(executor: &TaskToolExecutor) {
    let mut ops = executor.tracked_file_ops.lock().await;
    ops.push(FileOp::Create {
        path: "src/main.rs".to_string(),
        content: "fn main() {}".to_string(),
    });
    drop(ops);
    let mut sr = executor.self_review.lock().await;
    sr.record_write("src/main.rs");
    sr.record_read("src/main.rs");
}

#[tokio::test]
async fn task_done_passes_gate_when_tests_pass() {
    let runner = Arc::new(MockTestRunner::always_pass());
    let executor = make_executor_with_runner(runner.clone());
    seed_with_file_op(&executor).await;

    let calls = [task_done_call("done")];
    let results = executor.execute(&calls).await;

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].is_error,
        "task_done should pass when tests pass: {}",
        results[0].content
    );
    assert!(results[0].stop_loop);
    assert_eq!(*runner.calls.lock().await, 1, "test runner should be invoked exactly once");
    assert!(!*executor.dod_test_gate_exhausted.lock().await);
}

#[tokio::test]
async fn task_done_rejects_when_tests_fail_within_budget() {
    let runner = Arc::new(MockTestRunner::always_fail());
    let executor = make_executor_with_runner(runner.clone());
    seed_with_file_op(&executor).await;

    let calls = [task_done_call("done")];
    let results = executor.execute(&calls).await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(!results[0].stop_loop, "must keep iterating within budget");
    assert!(
        results[0]
            .content
            .contains("Definition-of-Done test gate"),
        "rejection prompt missing DoD framing: {}",
        results[0].content
    );
    assert!(
        results[0].content.contains("my_crate::tests::it_works"),
        "rejection prompt should list failing test names"
    );
    assert!(!*executor.dod_test_gate_exhausted.lock().await);
    assert_eq!(*executor.test_gate_attempts.lock().await, 1);
}

#[tokio::test]
async fn task_done_test_gate_marks_exhausted_after_budget() {
    let runner = Arc::new(MockTestRunner::always_fail());
    let executor = make_executor_with_runner(runner.clone());
    seed_with_file_op(&executor).await;

    // Hammer the gate. Each call increments test_gate_attempts; once it
    // reaches MAX_TASK_DONE_TEST_RETRIES the gate must flip to Exhausted
    // (stop_loop=true, dod_test_gate_exhausted=true).
    let mut last = None;
    for _ in 0..MAX_TASK_DONE_TEST_RETRIES {
        let results = executor.execute(&[task_done_call("done")]).await;
        last = Some(results);
    }

    let last = last.expect("at least one iteration");
    assert!(last[0].is_error);
    assert!(
        last[0].stop_loop,
        "exhausted budget must stop the agent loop"
    );
    assert!(
        last[0].content.contains("retry budget is exhausted"),
        "exhaustion prompt missing budget language: {}",
        last[0].content
    );
    assert!(
        *executor.dod_test_gate_exhausted.lock().await,
        "dod_test_gate_exhausted flag must be set"
    );
    assert_eq!(
        *executor.test_gate_attempts.lock().await,
        MAX_TASK_DONE_TEST_RETRIES
    );

    let mut result = TaskExecutionResult::default();
    executor.merge_into_result(&mut result).await;
    assert!(
        result.dod_test_gate_exhausted,
        "merge_into_result must propagate the exhausted flag"
    );
}

#[tokio::test]
async fn task_done_test_gate_skipped_when_no_command_or_default() {
    // /tmp/test isn't a real project root, so infer_default_test_command
    // returns None. With test_command also None the gate must skip rather
    // than block.
    let runner = Arc::new(MockTestRunner::always_pass());
    let executor = TaskToolExecutor {
        inner: Arc::new(NoOpInner),
        project_folder: "/this/path/definitely/does/not/exist".to_string(),
        build_command: None,
        test_command: None,
        task_context: String::new(),
        tracked_file_ops: Default::default(),
        notes: Default::default(),
        follow_ups: Default::default(),
        stub_fix_attempts: Default::default(),
        test_gate_attempts: Default::default(),
        test_runner: runner.clone(),
        disable_test_gate: false,
        task_phase: Arc::new(Mutex::new(TaskPhase::Implementing {
            plan: crate::planning::TaskPlan::empty(),
        })),
        self_review: Default::default(),
        event_tx: None,
        no_changes_needed: Default::default(),
        dod_test_gate_exhausted: Default::default(),
        recent_tool_outcomes: Default::default(),
    };
    seed_with_file_op(&executor).await;

    let results = executor.execute(&[task_done_call("done")]).await;
    assert!(!results[0].is_error);
    assert!(results[0].stop_loop);
    assert_eq!(
        *runner.calls.lock().await,
        0,
        "test runner must not be called when gate is skipped"
    );
}

#[tokio::test]
async fn task_done_test_gate_honors_disable_flag() {
    // The `disable_test_gate` field captures the env var at construction
    // time so the runtime gate check is just a struct read. Simulating
    // the operator opt-out is therefore a per-executor toggle rather than
    // a global env mutation that would race other tests.
    let runner = Arc::new(MockTestRunner::always_fail());
    let mut executor = make_executor_with_runner(runner.clone());
    executor.disable_test_gate = true;
    seed_with_file_op(&executor).await;

    let results = executor.execute(&[task_done_call("done")]).await;
    assert!(!results[0].is_error, "{}", results[0].content);
    assert!(results[0].stop_loop);
    assert_eq!(
        *runner.calls.lock().await,
        0,
        "test runner must not be called when the disable flag is set"
    );
}

#[test]
fn read_disable_test_gate_env_only_matches_one() {
    // Defence-in-depth on the env reader: only the literal "1" disables
    // the gate. Anything else (empty, "0", "true", "yes", typos) keeps
    // the gate live. This guards against an operator setting the var to
    // a truthy-looking value and being silently surprised.
    let prev = std::env::var(DISABLE_TEST_GATE_ENV).ok();
    for (val, expected) in [("1", true), ("0", false), ("true", false), ("", false)] {
        std::env::set_var(DISABLE_TEST_GATE_ENV, val);
        assert_eq!(
            super::read_disable_test_gate_env(),
            expected,
            "value {val:?} should map to {expected}"
        );
    }
    match prev {
        Some(v) => std::env::set_var(DISABLE_TEST_GATE_ENV, v),
        None => std::env::remove_var(DISABLE_TEST_GATE_ENV),
    }
}
