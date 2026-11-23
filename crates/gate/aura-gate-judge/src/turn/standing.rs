//! The review of work about to stand: hard work, or work that shows a
//! sign of failure, is gone through against its request before it
//! counts as done.

use aura_model::{ModelCallPurpose, ModelRequest, ReasonerError, ThinkingEffort};

use super::parts::{self, MAX_FAILURE};
use crate::criteria::{review_section, Criteria};
use crate::request::Ask;

/// Longest diff shown to the reviewer of work about to stand, in
/// characters: it has to see the whole change to say what is missing.
const MAX_REVIEW_DIFF: usize = 60_000;

/// System prompt of every review of work about to stand (see
/// [`aura_gate_followup`]).
pub const REVIEW_SYSTEM_PROMPT: &str = "\
You are reviewing an attempt at a task a user gave a coding agent, before the attempt is accepted. \
You see the user's request, what the attempt changed as a unified diff, the commands it ran, the \
last tests or builds it ran with what they printed, its final reply, and why it is being \
reviewed. Decide whether the work does what was asked.

Go through the request one requirement at a time: every behaviour, name, signature, message \
text, default, edge case and file it asks for. For each, find where the diff does it. A \
requirement the diff does not meet, meets only in part, or meets differently from what the \
request says (another name, message or default; a case left out; a file not touched that has to \
change) is a finding. Then read the diff for mistakes of its own: code that cannot work as \
written, a call to something that does not exist, a change that breaks what was there.

Do not take the attempt's word for it. Its reply says what it meant to do. A check that exited \
with success proves only what that check covers, and a test run that does not exercise the \
change says nothing about it. Failures the checks printed count against the work unless the diff \
plainly cannot have caused them.

- MODEL: there is at least one finding: the work is wrong or incomplete in a way that matters to \
what was asked. A more capable model would likely have done better.
- OK: every requirement is met as far as the diff shows and nothing in it looks broken. Missing \
tests, style, and things the request did not ask for are no reason to answer MODEL.
- OTHER: the work could not be done for reasons outside the attempt: the environment got in the \
way, or the request cannot be carried out as asked.

A finding has to name what the request asks for and what the diff does instead; when you have \
none, answer OK. List the findings, at most the five that matter most, as numbered points of a \
few lines each, concrete enough to act on without rereading the request: the file, the \
function, what is asked, what is there. Do not write up the requirements that are met or how \
you checked them. Then end with one line that is exactly `VERDICT: MODEL`, `VERDICT: OK` or \
`VERDICT: OTHER`.";

/// What the reviewer of work about to stand sees of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing<'a> {
    /// What the user asked.
    pub task: &'a str,
    /// What the attempt changed, as a unified diff; empty when it
    /// changed nothing.
    pub diff: &'a str,
    /// Whether `diff` holds every change: those made by commands it ran
    /// as well as those made with the file tools.
    pub whole: bool,
    /// The commands it ran, one per line, latest last.
    pub commands: &'a str,
    /// The last tests or builds it ran and what they printed; empty when
    /// it ran none.
    pub checks: &'a str,
    /// Its final reply.
    pub reply: &'a str,
    /// Why it is reviewed, one reason per entry.
    pub why: &'a [String],
    /// What it is checked against, written down before it started; the
    /// reviewer says how the work stands against each.
    pub criteria: Option<&'a Criteria>,
}

/// How the reviewer of work about to stand is asked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReviewVariant {
    /// With thinking off: one pass over the request and the diff.
    #[default]
    Plain,
    /// With room to reason before it answers.
    Thinking,
}

impl ReviewVariant {
    /// The variant's name as logged, e.g. `plain`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Thinking => "thinking",
        }
    }

    /// How hard the reviewer thinks before it answers.
    const fn effort(self) -> ThinkingEffort {
        match self {
            Self::Plain => ThinkingEffort::Off,
            Self::Thinking => ThinkingEffort::High,
        }
    }
}

/// The reviewer's message about `standing`. Nothing in it names the
/// model that did the work.
#[must_use]
pub fn review_prompt(standing: &Standing<'_>) -> String {
    let changes = if standing.whole {
        "## Changes (everything the attempt changed)"
    } else {
        "## Changes (made with the file tools; changes made by the commands it ran are not shown)"
    };
    let criteria = standing
        .criteria
        .map_or_else(String::new, |c| format!("{}\n\n", review_section(c)));
    format!(
        "## The user's request\n\n{}\n\n\
         {criteria}\
         {changes}\n```diff\n{}\n```\n\n\
         ## Commands it ran\n{}\n\n\
         ## Its last tests or builds\n{}\n\n\
         ## Final reply\n{}\n\n\
         ## Why it is reviewed\n{}",
        parts::task(standing.task),
        parts::changes(standing.diff, MAX_REVIEW_DIFF),
        parts::commands(standing.commands),
        parts::checks(standing.checks, 2 * MAX_FAILURE),
        parts::reply(standing.reply),
        parts::bullets(standing.why.iter().map(String::as_str)),
    )
}

/// The request that asks `model` to review `standing`, asked as
/// `variant` says and sent with the user's `auth_token` when the route
/// needs one.
///
/// # Errors
/// Returns the builder's error when `model` is empty.
pub fn review_request(
    model: &str,
    standing: &Standing<'_>,
    variant: ReviewVariant,
    auth_token: Option<String>,
) -> Result<ModelRequest, ReasonerError> {
    Ask::new(
        ModelCallPurpose::Judge,
        REVIEW_SYSTEM_PROMPT,
        review_prompt(standing),
    )
    .thinking(variant.effort())
    .request(model, auth_token)
}
