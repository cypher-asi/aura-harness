//! Test verification and auto-fix logic.
//!
//! Provides [`capture_test_baseline`] for recording pre-existing test failures
//! and [`run_and_handle_tests`] for executing the test suite with fix attempts.

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use tracing::{info, warn};

use crate::file_ops::FileOp;

use super::common::apply_fix_and_record;
use super::error_types::BuildFixAttemptRecord;
use super::runner::{self, IndividualTestResult};
use super::{emit, FixProvider, VerifyEvent};

/// Run the test suite and return the names of currently-failing tests.
///
/// Used before task execution to establish a baseline so that verification
/// can distinguish pre-existing failures from regressions introduced by the
/// current task.
pub async fn capture_test_baseline(project_root: &Path, test_command: &str) -> HashSet<String> {
    if test_command.trim().is_empty() {
        return HashSet::new();
    }
    match runner::run_build_command(project_root, test_command, None).await {
        Ok(result) => {
            let (tests, _) =
                runner::parse_test_output(&result.stdout, &result.stderr, result.success);
            let failures: HashSet<String> = tests
                .into_iter()
                .filter(|t| t.status == "failed")
                .map(|t| t.name)
                .collect();
            if !failures.is_empty() {
                info!(
                    count = failures.len(),
                    tests = ?failures,
                    "captured {} pre-existing test failure(s) as baseline",
                    failures.len(),
                );
            }
            failures
        }
        Err(e) => {
            warn!(error = %e, "baseline test capture failed, assuming no baseline");
            HashSet::new()
        }
    }
}

/// Run the test suite, compare against baseline, and attempt a fix if needed.
///
/// Returns `(tests_passed, input_tokens, output_tokens)`.
#[allow(clippy::too_many_arguments)]
pub async fn run_and_handle_tests(
    base_path: &Path,
    test_command: &str,
    attempt: u32,
    baseline_test_failures: &HashSet<String>,
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

    if test_result.success {
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

    // Check if all failures are pre-existing.
    if check_baseline_failures(&tests, baseline_test_failures) {
        let adjusted = format!(
            "{summary} ({} pre-existing, ignored)",
            tests.iter().filter(|t| t.status == "failed").count()
        );
        emit(
            event_tx,
            VerifyEvent::TestPassed {
                command: test_command.to_string(),
                stdout: test_result.stdout.clone(),
                summary: adjusted,
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

    Ok((false, inp, out))
}

fn check_baseline_failures(tests: &[IndividualTestResult], baseline: &HashSet<String>) -> bool {
    if baseline.is_empty() {
        return false;
    }
    let current_failures: HashSet<String> = tests
        .iter()
        .filter(|t| t.status == "failed")
        .map(|t| t.name.clone())
        .collect();
    let new_failures: Vec<&String> = current_failures
        .iter()
        .filter(|name| !baseline.contains(*name))
        .collect();
    if !new_failures.is_empty() {
        info!(
            new_failures = ?new_failures,
            pre_existing = current_failures.len() - new_failures.len(),
            "found {} new test failure(s) beyond baseline", new_failures.len()
        );
        return false;
    }
    info!(
        pre_existing = current_failures.len(),
        "all test failures are pre-existing (baseline), treating as passed"
    );
    true
}
