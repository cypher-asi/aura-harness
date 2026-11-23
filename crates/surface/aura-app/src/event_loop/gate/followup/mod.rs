//! What a turn Gate routed does when it ends with no check of its own
//! (see [`aura_gate::followup`]), shared by the terminal's turns,
//! `/compare` runs and `aura -p`.
//!
//! Each of them hands an attempt that has ended to [`Following::run`],
//! which asks [`FollowUp::next`] for each step and carries it out: a
//! nudge goes on in the same conversation on the same model, told what
//! [`CarryOn`] says; the harness check runs here; and so does the judge
//! of unchecked work, whose verdict is the outcome of work nothing else
//! checked. What either finds wrong sends the work on to a stronger
//! model as a failed check of the turn's own does. Each surface shows
//! what the steps find as its [`Report`] says.
//!
//! Work that is about to stand may be reviewed first
//! ([`FollowUp::review`]): the judge goes through the request against
//! everything the attempt changed. To show it that, the harness compares
//! the workspace with how it was before the attempt ([`Before`]): a
//! `/compare` run's copy against its baseline, a git working tree against
//! a snapshot taken when the turn started. Changes made by the commands
//! the attempt ran are then in the diff with those its file tools made.
//! Work the review finds unfinished goes back to its model once and on
//! to a stronger model after that.
//!
//! An attempt that failed, by its own check, the harness's or the
//! judge's verdict, goes on the same way from each surface: Gate's judge
//! says whether its model was at fault ([`Judging`]), and the next
//! stronger model takes the work over ([`Takeover`]).
//!
//! - `driver`: one pass of the follow-up over an attempt
//! - `report`: what a step found, shown as it is found
//! - `steps`: the steps an attempt may still take, and the next of them
//! - `review`: the review of work about to stand
//! - `judge`: the judge of work no check vouches for
//! - `check`: the harness's own check of a change
//! - `before`: the workspace before an attempt, and what it changed since
//! - `untested`: what an attempt added that no test names
//! - `messages`: what the model and the user are told of each step
//! - `ended`: the signal an attempt ended with, and how it failed
//! - `escalate`: the judge of a failure, and the move to a stronger model

mod before;
mod check;
mod driver;
mod ended;
mod escalate;
mod judge;
mod messages;
mod report;
mod review;
mod steps;
#[cfg(test)]
mod tests;
mod untested;

pub(crate) use before::Before;
use before::Changed;
pub(crate) use driver::{Attempt, CarryOn, Followed, Following};
pub(crate) use ended::{ended_signal, failure_text};
pub(crate) use escalate::{
    record_message, refused_notice, reroute, stronger, takeover_prompt, Judging, Takeover,
};
pub(crate) use messages::from_harness;
pub(crate) use report::{Found, Report};

use crate::event_loop::agents::AgentSession;
use aura_agent::AgentLoopResult;
use aura_config::ReviewMode;
use aura_core_types::{Difficulty, OutcomeSignal, PromptCategory};
use aura_gate::attempts::Tiers;
use aura_gate::followup::{Allowed, Taken};
use aura_gate::judge::criteria::Criteria;
use aura_gate::live::{self, GateLive};
use aura_gate::route::Candidate;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Whether a turn on `model`, routed among `candidates`, may have its
/// work reviewed before it stands, so the workspace is worth recording
/// before it starts.
pub(crate) fn may_review(live: &GateLive, candidates: &[Candidate], model: &str) -> bool {
    let settings = live.settings();
    (settings.review != ReviewMode::Off || settings.criteria != ReviewMode::Off)
        && live::reviewer_for(live, candidates, model).is_some()
}

/// How an attempt ended, for [`FollowUp::next`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct At {
    pub category: PromptCategory,
    /// How hard the classifier judged the work
    pub difficulty: Option<Difficulty>,
    /// Its signal so far; `None` while nothing says how it went
    pub signal: Option<OutcomeSignal>,
    /// Where the attempt's own messages start in its result
    pub turn_start: usize,
}

/// The steps one attempt at a routed turn may still take.
#[derive(Debug, Clone, Default)]
pub(crate) struct FollowUp {
    allowed: Allowed,
    taken: Taken,
    /// Test command the harness runs
    check: Option<String>,
    /// Model that gives the verdict on unchecked work
    judge: Option<String>,
    /// Model that reviews work about to stand
    reviewer: Option<String>,
    /// Files changed before the attempt was last sent on (asked to check
    /// its change, or sent back), as its file tools reported them
    changed_before: usize,
    /// The workspace before the attempt, when the harness recorded it
    before: Option<Before>,
    /// What each sign of failure has earned in the attempt log
    tiers: Tiers,
    /// What the attempt changed as of the run that left this many
    /// messages
    changed: Option<(usize, Option<Changed>)>,
    /// Where the attempt works, whose tests are searched for what it
    /// added
    workspace: PathBuf,
    /// What the attempt added that no test names, as of the run that
    /// left this many messages
    uncovered: Option<(usize, Vec<String>)>,
    /// What the work is checked against, written down before it started
    criteria: Option<Arc<Criteria>>,
}

/// Whether `result` stopped on a limit rather than by finishing.
fn hit_limit(result: &AgentLoopResult) -> bool {
    result.budget_exhausted || result.stalled || result.output_truncated
}

/// What the attempt `session`'s agent is running changed with its file
/// tools, as a unified diff whose paths are relative to `workspace`.
fn tool_edits(session: &AgentSession, workspace: &Path) -> String {
    (session.file_checkpoints.as_ref())
        .map(|files| files.attempt(session.agent_id()).diff(Some(workspace)))
        .unwrap_or_default()
}

/// `first`, then `second`, on lines of their own; whichever is present.
pub(crate) fn joined(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}\n{second}")),
        (first, second) => first.or(second),
    }
}
