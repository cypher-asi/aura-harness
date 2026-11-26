use super::*;

use crate::agent_runner::TaskExecutionResult;

use crate::verify::TestSuiteOutcome;

use std::sync::atomic::{AtomicBool, Ordering};

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

/// Test [`TaskTestRunner`] that returns a queue of pre-canned outcomes and
/// records every command it was invoked with.
///
/// Each call pops the next outcome; the queue is intentionally finite so a
/// runaway gate-loop in tests fails loudly with a panic instead of silently
/// reusing the last outcome. The `commands` log lets tests assert the gate
/// resolved the right test command (project config vs env override vs
/// inferred default).
#[derive(Debug, Default)]
struct MockTestRunner {
    queue: Mutex<Vec<Result<TestSuiteOutcome, BuildRunnerError>>>,
    calls: Mutex<u32>,
    commands: Mutex<Vec<String>>,
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
            commands: Mutex::new(Vec::new()),
        }
    }

    fn always_fail() -> Self {
        let mut q = Vec::new();
        for _ in 0..12 {
            q.push(Ok(TestSuiteOutcome {
                passed: false,
                summary: "9 passed, 1 failed".to_string(),
                failed_tests: vec!["my_crate::tests::it_works".to_string()],
                ..Default::default()
            }));
        }
        Self {
            queue: Mutex::new(q),
            calls: Mutex::new(0),
            commands: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl TaskTestRunner for MockTestRunner {
    async fn run_tests(
        &self,
        _project_root: &std::path::Path,
        command: &str,
    ) -> Result<TestSuiteOutcome, BuildRunnerError> {
        *self.calls.lock().await += 1;
        self.commands.lock().await.push(command.to_string());
        let mut q = self.queue.lock().await;
        if q.is_empty() {
            return Err(BuildRunnerError::Wait {
                command: command.to_string(),
                source: std::io::Error::other("MockTestRunner queue exhausted"),
            });
        }
        q.remove(0)
    }
}

#[derive(Debug, Default)]
struct HangingTestRunner;

#[async_trait::async_trait]
impl TaskTestRunner for HangingTestRunner {
    async fn run_tests(
        &self,
        _project_root: &std::path::Path,
        _command: &str,
    ) -> Result<TestSuiteOutcome, BuildRunnerError> {
        std::future::pending().await
    }
}

fn make_executor_with_runner(runner: Arc<dyn TaskTestRunner>) -> TaskToolExecutor {
    TaskToolExecutor {
        inner: Arc::new(NoOpInner),
        project_folder: "/tmp/test".to_string(),
        build_command: None,
        test_command: Some("cargo test --workspace".to_string()),
        test_command_override: None,
        task_context: String::new(),
        tracked_file_ops: Default::default(),
        notes: Default::default(),
        follow_ups: Default::default(),
        stub_fix_attempts: Default::default(),
        test_runner: runner,
        task_phase: Arc::new(Mutex::new(TaskPhase::Implementing {
            plan: crate::planning::TaskPlan::empty(),
        })),
        self_review: Default::default(),
        event_tx: None,
        no_changes_needed: Default::default(),
        recent_tool_outcomes: Default::default(),
        reset_explore_on_phase_change: Arc::new(AtomicBool::new(false)),
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

fn read_file_call(path: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: "read_1".to_string(),
        name: "read_file".to_string(),
        input: serde_json::json!({ "path": path }),
    }
}

/// Inner executor that denies every engine tool and logs which settle
/// path each call took.
#[derive(Default)]
struct DenyingInner {
    settled: std::sync::Mutex<Vec<(String, &'static str)>>,
}

#[async_trait::async_trait]
impl AgentToolExecutor for DenyingInner {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        tool_calls
            .iter()
            .map(|tc| ToolCallResult::success(&tc.id, "ok"))
            .collect()
    }

    fn spawn_preset<'a>(
        &'a self,
        call: ToolCallInfo,
        source: &'static str,
        result: ToolCallResult,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolCallResult> + Send + 'a>> {
        self.settled.lock().unwrap().push((call.name, source));
        Box::pin(std::future::ready(result))
    }

    fn spawn_local<'a>(
        &'a self,
        call: ToolCallInfo,
        source: &'static str,
        _run: std::pin::Pin<Box<dyn std::future::Future<Output = ToolCallResult> + Send + 'a>>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolCallResult> + Send + 'a>> {
        self.settled
            .lock()
            .unwrap()
            .push((call.name.clone(), source));
        Box::pin(std::future::ready(ToolCallResult::error(
            call.id,
            "Policy denied: task_done",
        )))
    }
}

// ------------------------------------------------------------------
// Codex parity: post-task_done best-effort test run tests
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

async fn recv_test_warning(
    rx: &mut tokio::sync::mpsc::Receiver<AgentLoopEvent>,
) -> (bool, String, Vec<String>) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Some(event) = tokio::time::timeout(remaining, rx.recv())
            .await
            .expect("expected TestSuiteWarning event before timeout")
        else {
            panic!("event channel closed before TestSuiteWarning");
        };
        if let AgentLoopEvent::TestSuiteWarning {
            passed,
            summary,
            failed_tests,
        } = event
        {
            return (passed, summary, failed_tests);
        }
    }
}

// ------------------------------------------------------------------
// Phase-reset handshake tests
// ------------------------------------------------------------------

/// Build an executor wired to a caller-supplied `Arc<AtomicBool>` so the
/// test can inspect the reset-signal flip directly.
fn make_executor_with_signal(signal: Arc<AtomicBool>) -> TaskToolExecutor {
    TaskToolExecutor {
        inner: Arc::new(NoOpInner),
        project_folder: "/tmp/test".to_string(),
        build_command: None,
        test_command: Some("cargo test --workspace".to_string()),
        test_command_override: None,
        task_context: String::new(),
        tracked_file_ops: Default::default(),
        notes: Default::default(),
        follow_ups: Default::default(),
        stub_fix_attempts: Default::default(),
        test_runner: Arc::new(MockTestRunner::always_pass()),
        task_phase: Arc::new(Mutex::new(TaskPhase::default())),
        self_review: Default::default(),
        event_tx: None,
        no_changes_needed: Default::default(),
        recent_tool_outcomes: Default::default(),
        reset_explore_on_phase_change: signal,
    }
}

mod completion;
mod outcomes;
mod test_runs;
mod write_first;
