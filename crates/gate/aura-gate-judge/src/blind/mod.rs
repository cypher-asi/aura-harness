//! The blind judge of `/compare` and the BENCH tab: a frontier model sees
//! one run of arm A and one of arm B (the task, each run's diff and final
//! reply) in random order, without arm or model names, and picks the
//! better one or calls a tie. Ported from `evals/gate_cost/judge.py`.
//!
//! When the work replays an earlier session, the judge also sees what the
//! user kept from it (its diff and last reply) as a reference, and rates
//! each attempt against it: better, as good, or worse.
//!
//! This module builds the request, decides the order and reads the
//! verdict; the caller sends the request through an agent's kernel, so
//! the call is recorded like any other model call.
//!
//! - `prompt`: what the judge is shown, in which order, and the request that shows it
//! - `verdict`: the winner and the reference ratings read out of its reply
//! - `unblind`: its reply with the attempts named as the runs they were

mod prompt;
#[cfg(test)]
mod tests;
mod unblind;
mod verdict;

pub(crate) use prompt::MAX_REPLY;
pub use prompt::{a_first, request, Attempt, MAX_DIFF, SYSTEM_PROMPT};
pub use unblind::unblind;
pub use verdict::{parse, reference_verdicts, reply_text, winner, Verdict};
