//! Whether the tests or build a turn ran last passed: the strongest
//! automatic signal of how a turn went.
//!
//! A check is a `run_command` call whose command line runs a test suite,
//! a build or a type check. Its result counts only when the command ran
//! to its end: a passing call, or a failing one that reports its exit
//! code. An exit status that may be another command's is decided by what
//! the check printed instead: a passing one when the check's output is
//! piped on or a command after it runs whatever it did (`pytest | tail`,
//! `pytest; echo done`), a failing one when any command follows the
//! check (`pytest | grep Error`, `pytest; pyflakes x`). Output that shows
//! the tests never ran (`no tests ran`) and none passed is a failure.
//! Calls the policy denied, that could not start, or that went to the
//! background say nothing, as does an exit status decided by output that
//! shows neither failures nor a pass.
//!
//! A check run with the change stashed away (`git stash; pytest; git
//! stash pop`) ran the code as it was before, so it is not the change's
//! check. When it shows failures, a failing run of the same check with
//! the change in place is not counted either: those failures were there
//! before.
//!
//! - `command`: which commands of a command line run a check, and what
//!   runs after the last one
//! - `stash`: which checks ran with the change stashed away
//! - `placed`: the checks of a turn, read out of its tool calls in order
//! - `verdict`: whether a check that ran to its end passed

use aura_model::Message;

mod command;
mod placed;
mod stash;
#[cfg(test)]
mod tests;
mod verdict;

pub use crate::transcript::command::command_line;
pub(crate) use command::exit_status_masked;
pub use command::is_check_part;
pub(crate) use placed::{Placed, Placing};

use placed::checks_placed;

/// A check that ran to its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSeen {
    /// The command line it ran.
    pub command: String,
    /// Whether it passed.
    pub passed: bool,
    /// Whether its exit status may have been another command's, so that
    /// what it printed decided whether it passed.
    pub by_output: bool,
    /// What it printed, as the model was shown it.
    pub output: String,
}

impl From<Placed> for CheckSeen {
    fn from(check: Placed) -> Self {
        Self {
            command: check.command,
            passed: check.passed,
            by_output: check.by_output,
            output: check.output.shown,
        }
    }
}

/// Whether the last check in `messages` passed; `None` when they hold no
/// check that ran to its end.
#[must_use]
pub fn last_check(messages: &[Message]) -> Option<bool> {
    checks_placed(messages).last().map(|check| check.passed)
}

/// The last check in `messages` that ran to its end, with its command
/// and output; `None` when they hold none.
#[must_use]
pub fn last_check_seen(messages: &[Message]) -> Option<CheckSeen> {
    checks_placed(messages).pop().map(CheckSeen::from)
}

/// Every check in `messages` that ran to its end, in the order they were
/// called.
#[must_use]
pub fn checks_seen(messages: &[Message]) -> Vec<CheckSeen> {
    checks_placed(messages)
        .into_iter()
        .map(CheckSeen::from)
        .collect()
}
