//! The judge of a failed turn, and which failed work it judges.

use aura_gate_route::Candidate;
use aura_model::model_catalog::infer_tier;
use aura_model::{ModelCallPurpose, ModelRequest, ReasonerError};

use super::parts::{self, MAX_FAILURE};
use crate::blind::MAX_DIFF;
use crate::request::{or_blank, tail, Ask};

/// System prompt of every request to the judge of failed turns; tells
/// them apart from the agent's own calls.
pub const SYSTEM_PROMPT: &str = "\
You are reviewing an attempt at a task a user gave a coding agent. The attempt failed: its last \
test or build command failed, or the run ended in an error. You see the user's request, the \
changes the attempt made to files as a unified diff (it may lack changes made through shell \
commands), its final reply, and how it failed. Decide what caused the failure:

- MODEL: the attempt's own work is wrong or incomplete. It misunderstood the request, made a \
wrong or partial change, or gave up. A more capable model would likely have done better.
- OTHER: the failure does not come from the attempt's work. The environment got in the way (a \
missing tool or dependency, no network, a timeout), the check was already failing for reasons \
unrelated to the request, the check ran before the fix and was not run again while the change \
itself is right, or the request cannot be carried out as asked.

When you cannot tell, answer MODEL. Say in one to three sentences what went wrong, concretely \
enough to help whoever takes the work over. Then end with one line that is exactly \
`VERDICT: MODEL` or `VERDICT: OTHER`.";

/// What the judge sees of a failed turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failed<'a> {
    /// What the user asked.
    pub task: &'a str,
    /// The turn's file changes as a unified diff; empty when it changed
    /// none.
    pub diff: &'a str,
    /// The turn's final reply.
    pub reply: &'a str,
    /// How it failed: the failing check and the end of its output, or
    /// the error.
    pub failure: &'a str,
}

/// The judge's message about `failed`. Nothing in it names the model
/// that did the work.
pub(super) fn prompt(failed: &Failed<'_>) -> String {
    format!(
        "## The user's request\n\n{}\n\n\
         ## Changes\n```diff\n{}\n```\n\n\
         ## Final reply\n{}\n\n\
         ## How it failed\n{}",
        parts::task(failed.task),
        parts::changes(failed.diff, MAX_DIFF),
        parts::reply(failed.reply),
        or_blank(failed.failure, "(nothing recorded)", |failure| {
            tail(failure, MAX_FAILURE)
        }),
    )
}

/// The request that asks `model` to judge `failed`, sent with the user's
/// `auth_token` when the route needs one.
///
/// # Errors
/// Returns the builder's error when `model` is empty.
pub fn request(
    model: &str,
    failed: &Failed<'_>,
    auth_token: Option<String>,
) -> Result<ModelRequest, ReasonerError> {
    Ask::new(ModelCallPurpose::Judge, SYSTEM_PROMPT, prompt(failed)).request(model, auth_token)
}

/// Whether `judge` reviews failed work done on `model`: only work on a
/// weaker tier than its own. Work on its tier has no stronger model to
/// go on to, and a model judging itself says little. Tiers come from
/// `candidates`, else from the models' names.
#[must_use]
pub fn judges(judge: &str, model: &str, candidates: &[Candidate]) -> bool {
    let tier = |id: &str| {
        candidates
            .iter()
            .find(|c| c.id == id)
            .map_or_else(|| infer_tier(id), |c| c.tier)
    };
    !judge.is_empty() && tier(model) < tier(judge)
}
