//! Verbatim renderers for every [`SteeringKind`] body.
//!
//! Each function returns the inner steering text WITHOUT the surrounding
//! `<harness_steering>` wrapper — wrapping is the injector's job (see
//! [`super::injector::SteeringInjector::render`]). Wording is preserved
//! bit-for-bit from the pre-PR-D inline call sites in `task_executor`.
//!
//! New variants land here as new private renderers; the dispatcher
//! [`render`] picks one per [`SteeringKind`] arm.

use super::SteeringKind;
use crate::file_ops::StubReport;
use crate::prompts::fix::build_stub_fix_prompt;

/// Dispatcher used by [`super::injector::SteeringInjector`]. Returns the
/// unwrapped body for `kind`; the injector layers the envelope on top.
#[must_use]
pub(super) fn render(kind: &SteeringKind) -> String {
    match kind {
        SteeringKind::TaskDoneNoWrites => task_done_no_writes(),
        SteeringKind::TaskDoneTestGateFailed {
            cmd,
            attempt,
            max_attempts,
            summary,
            failures_block,
            stderr_block,
        } => task_done_test_gate_failed(
            cmd,
            *attempt,
            *max_attempts,
            summary,
            failures_block,
            stderr_block,
        ),
        SteeringKind::TaskDoneTestGateExhausted {
            cmd,
            attempt,
            max_attempts,
            summary,
            failures_block,
            stderr_block,
        } => task_done_test_gate_exhausted(
            cmd,
            *attempt,
            *max_attempts,
            summary,
            failures_block,
            stderr_block,
        ),
        SteeringKind::TaskDoneTestGateIoFailure {
            cmd,
            error,
            attempt,
            max_attempts,
        } => task_done_test_gate_io_failure(cmd, error, *attempt, *max_attempts),
        SteeringKind::StubDetected { reports } => stub_detected(reports),
    }
}

fn task_done_no_writes() -> String {
    "ERROR: task_done was rejected — you have not produced any file changes \
     (write_file / edit_file / delete_file). Implementation tasks must produce \
     file changes. Make the edits this task requires, then call task_done. \
     If this task genuinely requires no file changes, call task_done again with \
     \"no_changes_needed\": true and explain why in the \"notes\" field."
        .to_string()
}

fn task_done_test_gate_failed(
    cmd: &str,
    attempt: usize,
    max_attempts: usize,
    summary: &str,
    failures_block: &str,
    stderr_block: &str,
) -> String {
    let header = format!(
        "ERROR: task_done blocked by Definition-of-Done test gate. \
         Running `{cmd}` reported failures (gate attempt {attempt}/{max_attempts}). \
         Fix EVERY failing test in the project — including tests that were already broken before \
         your task — then call task_done again.\n\nSummary: {summary}",
    );
    format!("{header}{failures_block}{stderr_block}")
}

fn task_done_test_gate_exhausted(
    cmd: &str,
    attempt: usize,
    max_attempts: usize,
    summary: &str,
    failures_block: &str,
    stderr_block: &str,
) -> String {
    let prompt = task_done_test_gate_failed(
        cmd,
        attempt,
        max_attempts,
        summary,
        failures_block,
        stderr_block,
    );
    format!(
        "{prompt}\n\nThis is attempt {attempt}/{max_attempts}. The \
         test gate retry budget is exhausted; the task is being marked as failed \
         with dod_test_gate_exhausted=true so the orchestrator can decide how to \
         proceed."
    )
}

fn task_done_test_gate_io_failure(
    cmd: &str,
    error: &str,
    attempt: usize,
    max_attempts: usize,
) -> String {
    format!(
        "ERROR: task_done test gate failed to execute `{cmd}`: {error}. \
         Fix the test command or your project setup, then call task_done \
         again. (gate attempt {attempt}/{max_attempts})"
    )
}

fn stub_detected(reports: &[StubReport]) -> String {
    build_stub_fix_prompt(reports)
}
