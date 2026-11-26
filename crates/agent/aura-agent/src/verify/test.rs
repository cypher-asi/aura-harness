//! Test verification helpers.
//!
//! Provides [`run_full_test_suite`] for running the project's test command and
//! reporting pass/fail status.
//!
//! Earlier revisions exposed a `capture_test_baseline` helper that recorded
//! pre-existing test failures so the agent loop could whitelist them. The
//! Definition-of-Done hard gate (Phase 6, 2026-04-25) deliberately removes
//! that whitelist: every task is now responsible for the full project suite,
//! so distinguishing "new" failures from "pre-existing" ones is no longer
//! something the verifier needs to do.

use std::path::Path;
use std::time::Instant;

use super::runner;

/// Outcome of running the full project test suite via [`run_full_test_suite`].
#[derive(Debug, Clone, Default)]
pub struct TestSuiteOutcome {
    /// `true` when the runner exited zero with no parsed failures.
    pub passed: bool,
    /// Short human-readable summary line (e.g. `"42 passed, 0 failed"`).
    pub summary: String,
    /// Names of tests that failed in this run. May be empty when the runner
    /// output is unparseable but the exit code was non-zero — in that case
    /// `passed` is still `false`.
    pub failed_tests: Vec<String>,
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
    let result = runner::run_build_command(project_root, test_command, None, None).await?;
    let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

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
        duration_ms,
    })
}
