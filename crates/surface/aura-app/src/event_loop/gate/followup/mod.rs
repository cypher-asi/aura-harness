//! What a turn Gate routed does when it ends with no check of its own
//! (see [`aura_gate::followup`]), shared by the terminal's turns,
//! `/compare` runs and `aura -p`.
//!
//! Each of them asks [`FollowUp::next`] once an attempt ends and carries
//! the step out: a nudge goes on in the same conversation on the same
//! model, told [`nudge_message`]; the harness check runs here
//! ([`harness_check`]); and so does the judge of unchecked work
//! ([`judge_unchecked`]), whose verdict is the outcome of work nothing
//! else checked. What either finds wrong sends the work on to a stronger
//! model as a failed check of the turn's own does.
//!
//! Work that is about to stand may be reviewed first
//! ([`FollowUp::review`]): the judge goes through the request against
//! everything the attempt changed. To show it that, the harness compares
//! the workspace with how it was before the attempt ([`Before`]): a
//! `/compare` run's copy against its baseline, a git working tree against
//! a snapshot taken when the turn started. Changes made by the commands
//! the attempt ran are then in the diff with those its file tools made.
//! Work the review finds unfinished goes back to its model once
//! ([`repair_message`]) and on to a stronger model after that.
//!
//! - `steps`: the steps an attempt may still take, and the next of them
//! - `review`: the review of work about to stand
//! - `judge`: the judge of work no check vouches for
//! - `check`: the harness's own check of a change
//! - `before`: the workspace before an attempt, and what it changed since
//! - `untested`: what an attempt added that no test names
//! - `messages`: what the model and the user are told of each step

mod before;
mod check;
mod judge;
mod messages;
mod review;
mod steps;
#[cfg(test)]
mod tests;
mod untested;

pub(crate) use before::Before;
use before::Changed;
pub(crate) use check::{check_review, check_signal, harness_check};
pub(crate) use judge::{judge_unchecked, Unchecked};
pub(crate) use messages::{
    nudge_message, nudge_notice, repair_message, repair_notice, sign_repair_message,
    sign_repair_notice,
};
pub(crate) use review::Reviewing;

use aura_agent::AgentLoopResult;
use aura_config::ReviewMode;
use aura_core_types::{Difficulty, OutcomeSignal, PromptCategory};
use aura_gate::attempts::Tiers;
use aura_gate::followup::{Allowed, Taken};
use aura_gate::judge::criteria::Criteria;
use aura_gate::live::{self, GateLive};
use aura_gate::route::Candidate;
use std::path::PathBuf;
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

/// `first`, then `second`, on lines of their own; whichever is present.
pub(crate) fn joined(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}\n{second}")),
        (first, second) => first.or(second),
    }
}
