//! Test verification helpers.
//!
//! Provides [`run_full_test_suite`] for running the project's test command and
//! reporting pass/fail status, plus [`run_and_handle_tests`] for the legacy
//! build-verify fix loop.
//!
//! Earlier revisions exposed a `capture_test_baseline` helper that recorded
//! pre-existing test failures so the agent loop could whitelist them. The
//! Definition-of-Done hard gate (Phase 6, 2026-04-25) deliberately removes
//! that whitelist: every task is now responsible for the full project suite,
//! so distinguishing "new" failures from "pre-existing" ones is no longer
//! something the verifier needs to do.

use std::path::Path;
use std::time::Instant;

use tracing::warn;

use crate::file_ops::FileOp;

use super::common::apply_fix_and_record;
use super::error_types::BuildFixAttemptRecord;
use super::runner;
use super::{emit, FixProvider, VerifyEvent};

/// Outcome of running the full project test suite via [`run_full_test_suite`].
#[derive(Debug, Clone, Default)]
pub struct TestSuiteOutcome {
    /// `true` when the runner exited zero with no parsed failures.
    pub passed: bool,
    /// Short human-readable summary line (e.g. `"42 passed, 0 failed"`).
    pub summary: String,
    /// Names of tests that failed in this run. May be empty when the runner
    /// output is unparseable but the exit code was non-zero — in that case
    /// `passed` is still `false` and callers should fall back to `raw_stderr`.
    pub failed_tests: Vec<String>,
    /// Captured stdout (truncated to the runner's standard output limit).
    pub raw_stdout: String,
    /// Captured stderr (truncated to the runner's standard output limit).
    pub raw_stderr: String,
    /// Wall-clock duration of the test run, milliseconds.
    pub duration_ms: u64,
}

/// Run the project's test command end-to-end and return a structured
/// [`TestSuiteOutcome`].
///
/// This is the single entry point used by the `task_done` hard gate; it is
/// intentionally side-effect-free: no fix attempts, no event emission, no
/// baseline whitelist. The caller decides what to do with the outcome
/// (re-prompt the agent, exhaust the retry budget, etc.).
pub async fn run_full_test_suite(
    project_root: &Path,
    test_command: &str,
) -> anyhow::Result<TestSuiteOutcome> {
    if test_command.trim().is_empty() {
        anyhow::bail!("run_full_test_suite called with empty test_command");
    }
    let start = Instant::now();
    let result = runner::run_build_command(project_root, test_command, None).await?;
    let duration_ms = start.elapsed().as_millis() as u64;

    let (tests, summary) =
        runner::parse_test_output(&result.stdout, &result.stderr, result.success);

    let failed_tests: Vec<String> = tests
        .iter()
        .filter(|t| t.status == "failed")
        .map(|t| t.name.clone())
        .collect();

    let passed = result.success && failed_tests.is_empty();

    Ok(TestSuiteOutcome {
        passed,
        summary,
        failed_tests,
        raw_stdout: result.stdout,
        raw_stderr: result.stderr,
        duration_ms,
    })
}

/// Run the test suite and attempt a fix if needed.
///
/// Used by the legacy [`super::build::verify_and_fix_build`] loop. Returns
/// `(tests_passed, input_tokens, output_tokens)`.
#[allow(clippy::too_many_arguments)]
pub async fn run_and_handle_tests(
    base_path: &Path,
    test_command: &str,
    attempt: u32,
    fix_provider: &dyn FixProvider,
    event_tx: Option<&tokio::sync::mpsc::UnboundedSender<VerifyEvent>>,
    prior_test_attempts: &mut Vec<BuildFixAttemptRecord>,
    all_fix_ops: &mut Vec<FileOp>,
) -> anyhow::Result<(bool, u64, u64)> {
    emit(
        event_tx,
        VerifyEvent::TestStarted {
            command: test_command.to_string(),
        },
    );

    let test_start = Instant::now();
    let test_result = runner::run_build_command(base_path, test_command, None).await?;
    let dur = test_start.elapsed().as_millis() as u64;

    let (tests, summary) = runner::parse_test_output(
        &test_result.stdout,
        &test_result.stderr,
        test_result.success,
    );

    let failure_count = tests.iter().filter(|t| t.status == "failed").count();
    if test_result.success && failure_count == 0 {
        emit(
            event_tx,
            VerifyEvent::TestPassed {
                command: test_command.to_string(),
                stdout: test_result.stdout.clone(),
                summary: summary.clone(),
                duration_ms: dur,
            },
        );
        return Ok((true, 0, 0));
    }

    emit(
        event_tx,
        VerifyEvent::TestFailed {
            command: test_command.to_string(),
            stdout: test_result.stdout.clone(),
            stderr: test_result.stderr.clone(),
            attempt,
            summary: summary.clone(),
            duration_ms: dur,
        },
    );
    emit(event_tx, VerifyEvent::TestFixAttempt { attempt });

    let (response, inp, out) = fix_provider
        .request_fix(
            test_command,
            &test_result.stderr,
            &test_result.stdout,
            prior_test_attempts,
        )
        .await?;

    apply_fix_and_record(
        base_path,
        &response,
        attempt,
        &test_result.stderr,
        prior_test_attempts,
        all_fix_ops,
        "test-fix",
        fix_provider,
    )
    .await?;

    if tests.is_empty() {
        warn!("test runner produced no parseable test results; relying on exit code");
    }

    Ok((false, inp, out))
}
