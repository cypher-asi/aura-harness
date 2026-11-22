//! Layer: gate
//!
//! Where Gate routes work: the candidates the user's sign-ins allow, the
//! model a turn or subtask goes to, what Gate has learned of each model,
//! and how another way of routing would have done on the recorded work.
//!
//! - [`candidates`], [`pick`] and [`escalate`]: the models Gate may route
//!   to, the one work starts on, and the one it goes on with after a
//!   failed attempt
//! - [`stats`]: what Gate has learned per level, category and model,
//!   built from the record
//! - [`replay`]: how another way of routing would have done on the
//!   recorded work
//!
//! Gate never calls a model itself: it chooses which model a turn runs
//! on, and the calls go through the agent's kernel as usual.

pub mod replay;
mod router;
pub mod stats;

pub use router::{
    below_lead, by_expected_cost, candidates, current, escalate, fixed_pick, has_stronger_model,
    pick, Candidate, Decision, Random, SplitMix64, Work,
};
