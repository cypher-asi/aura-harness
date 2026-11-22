//! Gate's acceptance criteria: what a routed code turn will be checked
//! against, written down by a model other than the one doing the work
//! before the work starts.
//!
//! A check that exits cleanly proves only what it covers, and the worker
//! writes its own tests, so a request's exact demands (an output format,
//! a reference implementation to match, the error a bad input raises) can
//! go unchecked while every test passes. One call reads the whole request
//! and lists those demands; the review of work about to stand
//! ([`aura_gate_followup`], [`crate::turn::review_prompt`]) goes through
//! them one at a time, and finds out which are met, which are not and
//! which nothing the attempt ran shows. The worker cannot change the
//! list: it is kept by the harness, not in the workspace.
//!
//! This module builds the request and reads the reply; the caller sends
//! the request through its agent's kernel, so the call is recorded like
//! any other model call.
//!
//! - `kind`: what a criterion asks for
//! - `list`: the criteria a request's work is held to, and the note the worker gets of them
//! - `prompt`: the request for criteria, and the tool its writer answers with
//! - `answer`: the criteria read out of the writer's reply, or why there are none
//! - `review`: how the reviewer is asked about each criterion, and what it found

mod answer;
mod kind;
mod list;
mod prompt;
mod review;
#[cfg(test)]
mod tests;

use aura_gate_route::{fixed_pick, Candidate};

pub use answer::{criteria_of, none_because};
pub use kind::Kind;
pub use list::{worker_note, Criteria, Criterion};
pub use prompt::{request, CRITERIA_TOOL, SYSTEM_PROMPT};
pub(crate) use review::review_section;
pub use review::{checked, Checked, Status};

/// Most criteria kept: more than this many are rarely all checkable, and
/// the review has to go through each.
const MAX_ITEMS: usize = 12;

/// Exacting criteria ([`Kind::exacting`]) that make work hard even when
/// the writer did not call the request strict.
const EXACTING_ITEMS: usize = 6;

/// The model that writes a turn's criteria among `candidates`: the
/// cheapest standard-tier one, else the strongest ([`fixed_pick`]).
/// Reading a long request for what it demands is not work for the
/// cheapest model. `None` when there are no candidates.
#[must_use]
pub fn writer(candidates: &[Candidate]) -> Option<&Candidate> {
    fixed_pick(candidates)
}
