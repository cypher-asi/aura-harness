//! The judge of a failed turn: a strong model sees what the user asked,
//! what the turn changed, its final reply and how it failed, and says
//! whether the model that did the work was at fault.
//!
//! Gate only calls it for work that already failed (an error, or its
//! last test or build failing), so its cost is paid when there is
//! something to learn. A failure the judge puts down to the model counts
//! against it and sends the work on to a stronger one; a failure it puts
//! down to something else (the environment, a check that was failing
//! anyway) counts for nothing and the work stays where it is.
//!
//! The same judge gives the verdict on work that ran no check (see
//! [`aura_gate_followup`]), through [`unverified_request`]: there it may
//! also find the work done (`OK`). And it reviews work that is about to
//! stand, through [`review_request`]: hard work, or work that shows a
//! sign of failure the attempt log backs, is gone through against the
//! request before it counts as done.
//!
//! This module builds the request and reads the verdict; the caller
//! sends the request through the agent's kernel, so the call is recorded
//! like any other model call.
//!
//! - `failed`: the judge of a failed turn, and which work it judges
//! - `unverified`: the judge of a turn no check vouches for
//! - `standing`: the review of work about to stand
//! - `parts`: the parts of a turn its judges are shown, cut to their limits
//! - `verdict`: what a judge answered, read out of its reply

mod failed;
mod parts;
mod standing;
#[cfg(test)]
mod tests;
mod unverified;
mod verdict;

pub use failed::{judges, request, Failed, SYSTEM_PROMPT};
pub use standing::{review_prompt, review_request, ReviewVariant, Standing, REVIEW_SYSTEM_PROMPT};
pub use unverified::{unverified_request, Unverified, UNVERIFIED_SYSTEM_PROMPT};
pub use verdict::{parse_answer, Answer, Verdict};
