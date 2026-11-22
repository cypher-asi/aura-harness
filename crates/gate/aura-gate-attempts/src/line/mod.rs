//! The lines of the log: an attempt, a truth about one, or a review of
//! one, told apart on disk by their `kind`.

mod attempt;
mod review;
mod truth;

pub use attempt::{Attempt, Terms};
pub use review::Review;
pub use truth::{CopyAt, Truth, TruthSource};

use serde::{Deserialize, Serialize};

/// What kind a line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// See [`Attempt`]
    Attempt,
    /// See [`Truth`]
    Truth,
    /// See [`Review`]
    Review,
}

/// A line of the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Line {
    /// See [`Attempt`]
    Attempt(Attempt),
    /// See [`Truth`]
    Truth(Truth),
    /// See [`Review`]
    Review(Review),
}

impl Line {
    /// The agent and prompt the line is about, and its kind.
    #[must_use]
    pub fn key(&self) -> (&str, &str, Kind) {
        match self {
            Self::Attempt(a) => (&a.agent, &a.prompt_tx, Kind::Attempt),
            Self::Truth(t) => (&t.agent, &t.prompt_tx, Kind::Truth),
            Self::Review(r) => (&r.agent, &r.prompt_tx, Kind::Review),
        }
    }
}

#[cfg(test)]
mod tests;
