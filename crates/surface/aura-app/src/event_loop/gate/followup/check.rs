//! The harness's check of a change: the project's tests, run when the
//! attempt ran none of its own.

use crate::event_loop::agents::AgentSession;
use crate::event_loop::task_check::{run_script_within, CheckRun};
use aura_core_types::OutcomeSignal;
use std::path::Path;

/// `ToolProposal::source` of the harness's checks.
const CHECK_SOURCE: &str = "gate_check";

/// Longest the harness's check may run. One that takes longer says
/// nothing, so a slow suite costs the user at most this long.
const CHECK_TIMEOUT_SECS: u64 = 5 * 60;

/// Exit codes of a command the shell could not run (126: not
/// executable, 127: not found).
const NOT_RUN_EXIT_CODES: [i32; 2] = [126, 127];

/// Exit codes of pytest that say nothing about the change: 4 a usage
/// error, 5 no tests collected.
const PYTEST_NOT_RUN_EXIT_CODES: [i32; 2] = [4, 5];

/// What a check printed when its tools are missing, not its tests
/// failing.
const NOT_RUN_OUTPUT: &[&str] = &[
    "command not found",
    "No module named pytest",
    "Missing script",
    "is not recognized as",
];

/// The test command the harness runs in `workspace`: the operator's
/// (`AURA_DOD_TEST_COMMAND`), else the one its manifest implies.
pub(super) fn check_command(workspace: &Path) -> Option<String> {
    aura_config::agent()
        .verify
        .test_command_override
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty())
        .or_else(|| aura_agent::infer_default_test_command(workspace))
}

/// Run `command` in `session`'s workspace as the harness's check.
pub(crate) async fn harness_check(session: &AgentSession, command: &str) -> CheckRun {
    run_script_within(session, command, CHECK_SOURCE, CHECK_TIMEOUT_SECS).await
}

/// What `check` says of the change: passed or failed, `None` when it
/// could not run, ran out of time, or failed for want of its tools.
pub(crate) fn check_signal(check: &CheckRun) -> Option<OutcomeSignal> {
    if could_not_run(check) {
        return None;
    }
    check.signal()
}

fn could_not_run(check: &CheckRun) -> bool {
    match (check.passed, check.exit_code) {
        (None, _) | (Some(false), None) => true,
        (Some(true), _) => false,
        (Some(false), Some(code)) => {
            NOT_RUN_EXIT_CODES.contains(&code)
                || (check.command.contains("pytest") && PYTEST_NOT_RUN_EXIT_CODES.contains(&code))
                || NOT_RUN_OUTPUT.iter().any(|text| check.tail.contains(text))
        }
    }
}

/// What the stronger model is told of a harness check that failed.
pub(crate) fn check_review(check: &CheckRun) -> String {
    format!("The harness ran the project's tests. {}", check.describe())
}
