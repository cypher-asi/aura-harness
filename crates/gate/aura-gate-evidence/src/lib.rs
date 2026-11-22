//! Layer: gate
//!
//! What an attempt Gate routed left behind, and what it says of how the
//! attempt went.
//!
//! - [`transcript`]: reading a turn's messages: its tool calls, what its
//!   commands printed, what its reply says, and Gate's own messages
//! - [`checks`]: whether the tests or build a turn ran last passed
//! - [`own_check`]: whether a turn ran the code it changed itself, where
//!   no test or build checked it
//! - [`coverage`]: what a change adds, to tell whether the tests that
//!   passed could have checked it
//! - [`outcome`]: the signals that say how a prompt's work went, and
//!   which one counts
//! - [`Evidence`] and [`flags`]: what an attempt left behind, and the
//!   signs of a failed attempt read out of it
//! - [`verifiable`]: which kinds of work a check verifies
//! - `test_support`: builders of a turn's messages, for tests (feature
//!   `test-support`)

#![deny(clippy::too_many_lines)]

mod category;
pub mod checks;
pub mod coverage;
mod evidence;
pub mod outcome;
pub mod own_check;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod transcript;

pub use category::verifiable;
pub use evidence::{flags, is_test_path, CheckEvidence, Evidence, Flag};
