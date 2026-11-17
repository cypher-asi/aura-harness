//! Layer: agent
//!
//! Gate, the router behind the `gate` model id.
//!
//! - [`router`]: the candidates the user's sign-ins allow and the model a
//!   turn goes to
//! - [`classify`]: the request that labels a prompt with its category,
//!   and reading the reply
//! - [`checks`]: whether the tests or build a turn ran last passed
//! - [`outcome`]: the signals that say how a prompt's work went, and
//!   which one counts
//!
//! Gate never calls a model itself: it chooses which model a turn runs
//! on and builds the classifier's request, and the calls go through the
//! agent's kernel as usual.

pub mod checks;
pub mod classify;
pub mod outcome;
pub mod router;

pub use router::{candidates, fixed_pick, pick_order, Candidate};
