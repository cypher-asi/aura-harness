//! Layer: agent
//!
//! Gate, the router behind the `gate` model id.
//!
//! - [`router`]: the candidates the user's sign-ins allow and the model
//!   a turn or subtask goes to
//! - [`stats`]: what Gate has learned per level, category and model,
//!   built from the record
//! - [`classify`]: the request that labels a prompt with its category,
//!   and reading the reply
//! - [`criteria`]: the acceptance criteria a routed code turn is written
//!   before its work starts, and what the review found of each
//! - [`checks`]: whether the tests or build a turn ran last passed
//! - [`own_check`]: whether a turn ran the code it changed itself, where
//!   no test or build checked it
//! - [`followup`]: what Gate does about a routed turn that ran no check:
//!   ask it to verify, run the tests itself, or have the judge review it
//! - [`outcome`]: the signals that say how a prompt's work went, and
//!   which one counts
//! - [`compare`]: `/compare`'s arguments, schedule, estimate and the
//!   per-arm summary it shows
//! - [`judge`]: the blind judge of `/compare`, which picks the better of
//!   two runs
//! - [`turn_judge`]: the judge of a failed turn, which says whether the
//!   model was at fault
//! - [`lead`]: what the lead agent under Gate may do itself while
//!   delegation is enforced
//! - [`delegation`]: how much of a lead's work went to subtasks
//! - [`replay`]: how another way of routing would have done on the
//!   recorded work
//! - [`evidence`]: what an attempt left behind that says how it went,
//!   and the signs of a failed attempt read out of it
//! - [`attempt_log`]: the local log of attempts and what later proved
//!   them right or wrong, and the report that scores Gate and each sign
//!   against it
//! - [`coverage`]: what a change adds, to tell whether the tests that
//!   passed could have checked it
//!
//! Gate never calls a model itself: it chooses which model a turn runs
//! on and builds the classifier's and judge's requests, and the calls go
//! through the agent's kernel as usual.

pub mod attempt_log;
pub mod checks;
pub mod classify;
pub mod compare;
pub mod coverage;
pub mod criteria;
pub mod delegation;
pub mod evidence;
pub mod followup;
pub mod judge;
pub mod lead;
pub mod outcome;
pub mod own_check;
pub mod replay;
pub mod router;
pub mod stats;
pub mod turn_judge;

pub use lead::{choose_way, Way};
pub use router::{
    below_lead, candidates, effort_on, escalate, fixed_pick, has_stronger_model, judged, pick,
    raise_effort, start_effort, Candidate, Decision, Random, SplitMix64, Work,
};
pub use stats::GateStats;
