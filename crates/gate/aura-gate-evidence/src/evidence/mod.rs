//! What an attempt left behind that says how it went, before anything
//! outside it checks the work: the checks it ran and what they printed,
//! what it changed, and what it said at the end.
//!
//! [`Evidence`] is the raw material, kept in Gate's attempt log
//! (`aura-gate-attempts`). [`flags`] reads signs of a failed attempt out
//! of it. The flags are computed when the log is evaluated, not when it
//! is written, so a new one is scored on every attempt already logged.
//!
//! - `types`: [`Evidence`] and [`CheckEvidence`], as the log keeps them
//! - `scan`: [`Evidence::of`], which reads them in one pass over an
//!   attempt's messages
//! - `flag`: [`Flag`], a sign of a failed attempt
//! - `flags`: [`flags`], the signs an attempt's evidence raises

mod flag;
mod flags;
mod scan;
#[cfg(test)]
mod tests;
mod types;

pub use flag::Flag;
pub use flags::{flags, is_test_path};
pub use types::{CheckEvidence, Evidence};
