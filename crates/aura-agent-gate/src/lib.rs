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
//! - [`checks`]: whether the tests or build a turn ran last passed
//! - [`outcome`]: the signals that say how a prompt's work went, and
//!   which one counts
//! - [`compare`]: `/compare`'s arguments, schedule, estimate and the
//!   per-arm summary it shows
//! - [`judge`]: the blind judge of `/compare`, which picks the better of
//!   two runs
//!
//! Gate never calls a model itself: it chooses which model a turn runs
//! on and builds the classifier's and judge's requests, and the calls go
//! through the agent's kernel as usual.

pub mod checks;
pub mod classify;
pub mod compare;
pub mod judge;
pub mod outcome;
pub mod router;
pub mod stats;

pub use router::{candidates, fixed_pick, pick, Candidate, Decision, Random, SplitMix64};
pub use stats::GateStats;
