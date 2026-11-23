//! The judge of a turn no check vouches for: its verdict is the
//! attempt's outcome.

use std::borrow::Cow;

use aura_gate_followup::Suspicion;
use aura_model::{ModelCallPurpose, ModelRequest, ReasonerError};

use super::parts::{self, MAX_FAILURE};
use crate::blind::MAX_DIFF;
use crate::request::{or_blank, Ask};

/// System prompt of every request to the judge of unverified turns:
/// attempts that no check vouches for, because they ran none or because
/// their own reply says the work was not verified (see
/// [`aura_gate_followup`]). Its verdict is the attempt's outcome.
pub const UNVERIFIED_SYSTEM_PROMPT: &str = "\
You are reviewing an attempt at a task a user gave a coding agent. Nothing shows that its work \
is right: it ran no test or build that checks the work, or it ran one and its own final reply \
says the work was not verified. The task may ask for a code change, an answer, research, a plan \
or a piece of writing. You see the user's request, the changes the attempt made to files as a \
unified diff (it may lack changes made through shell commands; the commands it ran are listed), \
the last test or build it ran with what that printed when it ran one, its final reply, and \
anything that looks wrong about it. Decide whether the work is done:

- MODEL: the attempt's own work is wrong, incomplete or missing. It misunderstood the request, \
made a wrong or partial change, changed nothing when a change was needed, answered something \
other than what was asked, got facts wrong, or gave up. A more capable model would likely have \
done better.
- OK: the work looks complete and correct for what was asked, or no change was needed and the \
reply shows why. A reply that only names something beside the point it left untested (another \
platform, an optional style check, tests the request did not ask for) is no reason to doubt \
work that is otherwise right.
- OTHER: the work could not be done for reasons outside the attempt: the environment got in the \
way, or the request cannot be carried out as asked.

Judge the work itself, not how it was described. Say in one to three sentences what you found, \
concretely enough to help whoever takes the work over. Then end with one line that is exactly \
`VERDICT: MODEL`, `VERDICT: OK` or `VERDICT: OTHER`.";

/// What the judge sees of a turn no check vouches for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unverified<'a> {
    /// What the user asked.
    pub task: &'a str,
    /// The turn's file changes as a unified diff; empty when it changed
    /// none.
    pub diff: &'a str,
    /// The turn's final reply.
    pub reply: &'a str,
    /// The commands it ran, one per line, latest last.
    pub commands: &'a str,
    /// The last test or build it ran and what that printed; empty when
    /// it ran none.
    pub last_check: &'a str,
    /// What looks wrong; empty when nothing does.
    pub suspicions: &'a [Suspicion],
}

/// The judge's message about `unverified`. Nothing in it names the model
/// that did the work.
pub(super) fn unverified_prompt(unverified: &Unverified<'_>) -> String {
    let wrong = parts::bullets(unverified.suspicions.iter().map(|s| s.describe()));
    format!(
        "## The user's request\n\n{}\n\n\
         ## Changes\n```diff\n{}\n```\n\n\
         ## Commands it ran\n{}\n\n\
         ## Its last test or build\n{}\n\n\
         ## Final reply\n{}\n\n\
         ## What looks wrong\n{}",
        parts::task(unverified.task),
        parts::changes(unverified.diff, MAX_DIFF),
        parts::commands(unverified.commands),
        parts::checks(unverified.last_check, MAX_FAILURE),
        parts::reply(unverified.reply),
        or_blank(&wrong, "(nothing in particular)", Cow::Borrowed),
    )
}

/// The request that asks `model` to judge `unverified`, sent with the
/// user's `auth_token` when the route needs one.
///
/// # Errors
/// Returns the builder's error when `model` is empty.
pub fn unverified_request(
    model: &str,
    unverified: &Unverified<'_>,
    auth_token: Option<String>,
) -> Result<ModelRequest, ReasonerError> {
    let message = unverified_prompt(unverified);
    Ask::new(ModelCallPurpose::Judge, UNVERIFIED_SYSTEM_PROMPT, message).request(model, auth_token)
}
