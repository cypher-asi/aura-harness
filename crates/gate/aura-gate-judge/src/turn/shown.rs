//! What a turn's judges are shown of the commands it ran: the latest of
//! them, one per line, and the tests or builds among them with what they
//! printed.

use aura_gate_evidence::checks::{checks_seen, last_check_seen, CheckSeen};
use aura_gate_evidence::transcript::command_lines;
use aura_model::Message;

/// Commands shown to the judge of unchecked work, latest last.
const JUDGED_COMMANDS: usize = 15;

/// Commands shown to the reviewer of work about to stand, latest last.
const REVIEWED_COMMANDS: usize = 40;

/// Checks shown to the reviewer of work about to stand, latest last.
const REVIEWED_CHECKS: usize = 3;

/// What the judge of unchecked work is shown of the commands in
/// `messages`: the last of those they ran, one per line, and their last
/// test or build (empty when they ran none).
#[must_use]
pub fn shown_to_judge(messages: &[Message]) -> (String, String) {
    let last_check = last_check_seen(messages)
        .map(|check| check_shown(&check))
        .unwrap_or_default();
    (last_commands(messages, JUDGED_COMMANDS), last_check)
}

/// What the reviewer of work about to stand is shown of the commands in
/// `messages`: the last of those they ran, one per line, and their last
/// few tests or builds, each with what it printed (empty when they ran
/// none).
#[must_use]
pub fn shown_to_reviewer(messages: &[Message]) -> (String, String) {
    let seen = checks_seen(messages);
    let checks = seen[seen.len().saturating_sub(REVIEWED_CHECKS)..]
        .iter()
        .map(check_shown)
        .collect::<Vec<_>>()
        .join("\n\n");
    (last_commands(messages, REVIEWED_COMMANDS), checks)
}

/// The command lines of the last `shown` `run_command` calls in
/// `messages`, one per line.
fn last_commands(messages: &[Message], shown: usize) -> String {
    let commands: Vec<String> = command_lines(messages).collect();
    commands[commands.len().saturating_sub(shown)..].join("\n")
}

/// A check as a turn's judges see it: its command, how it exited, and
/// what it printed. A command whose exit status may not be the check's
/// is pointed out, since only the output then says how the check went.
pub(super) fn check_shown(check: &CheckSeen) -> String {
    let exited = match (check.passed, check.by_output) {
        (true, false) => "exited with success",
        (false, false) => "failed",
        (true, true) => {
            "may have exited as a command after the check did, so what the check printed \
             decided how it went: it shows tests passing"
        }
        (false, true) => {
            "may have exited as a command after the check did, so what the check printed \
             decided how it went: it shows failures, or that the tests never ran"
        }
    };
    format!(
        "`{}` {exited}. It printed:\n{}",
        check.command, check.output
    )
}
