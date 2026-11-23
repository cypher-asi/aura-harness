//! What the blind judge is shown, in which order, and the request that
//! shows it.

use std::borrow::Cow;

use aura_gate_route::{Random, SplitMix64};
use aura_model::{ModelCallPurpose, ModelRequest, ReasonerError};

use crate::request::{cut, or_blank, Ask};

/// Longest diff the judge sees per attempt, in characters.
pub const MAX_DIFF: usize = 12_000;

/// Longest final reply the judge sees per attempt, in characters.
pub(crate) const MAX_REPLY: usize = 4_000;

/// What `a_first` mixes a pair's index with, so that neighbouring pairs
/// get unrelated orders.
const PAIR_MIX: u64 = 0x9E37_79B9_7F4A_7C15;

/// System prompt of every judge request; tells them apart from the
/// agent's own calls.
pub const SYSTEM_PROMPT: &str = "\
You are reviewing two attempts at the same coding task, made by different agents in identical \
copies of the same repository. Both may have passed the task's automated checks, so judge what \
the checks miss:

- correctness and completeness against everything the user asked, including edge cases;
- scope: no changes nobody asked for;
- code quality: clear, idiomatic, consistent with the surrounding code;
- the final reply: accurate about what was done, no false claims, useful to the user.

Explain your judgment in two to four sentences, naming the concrete differences that decided \
it. Always call them \"Attempt A\" and \"Attempt B\" in full, never a bare A or B. Then end with one line that is exactly `WINNER: A`, `WINNER: B` or `WINNER: TIE` (TIE only \
when neither is meaningfully better).";

/// What the judge is told when there is a reference.
const REFERENCE_INSTRUCTIONS: &str = "\
The reference below is what the user kept when this work was first done: a known-acceptable \
result, not necessarily the only correct one. Besides the winner, rate each attempt against it. \
Before the WINNER line, add two lines, each exactly `A_VS_REFERENCE: BETTER`, `AS_GOOD` or \
`WORSE`, and the same for `B_VS_REFERENCE`.";

/// What the judge sees of one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    /// The run's changes as a unified diff; empty when it changed nothing.
    pub diff: String,
    /// The run's final reply.
    pub reply: String,
}

impl Attempt {
    /// The changes as the judge sees them.
    fn changes(&self) -> Cow<'_, str> {
        or_blank(&self.diff, "(no changes)", |diff| {
            cut(diff, MAX_DIFF, "diff")
        })
    }

    /// The final reply as the judge sees it.
    fn final_reply(&self) -> Cow<'_, str> {
        or_blank(&self.reply, "(no reply)", |reply| {
            cut(reply, MAX_REPLY, "reply")
        })
    }
}

/// The judge's message for the attempts at `task`, `first` shown as A,
/// with what the user kept from the original work when there is a
/// `reference`. Nothing in it says which arm or model made either
/// attempt.
pub(super) fn prompt(
    task: &str,
    first: &Attempt,
    second: &Attempt,
    reference: Option<&Attempt>,
) -> String {
    let reference = reference.map_or_else(String::new, |reference| {
        format!(
            "{REFERENCE_INSTRUCTIONS}\n\n## Reference (what the user kept)\n\n\
             ### Changes\n```diff\n{}\n```\n\n### Final reply\n{}\n\n",
            reference.changes(),
            reference.final_reply(),
        )
    });
    format!(
        "## The user's message\n\n{task}\n\n{reference}\
         ## Attempt A\n\n### Changes\n```diff\n{}\n```\n\n### Final reply\n{}\n\n\
         ## Attempt B\n\n### Changes\n```diff\n{}\n```\n\n### Final reply\n{}",
        first.changes(),
        first.final_reply(),
        second.changes(),
        second.final_reply(),
    )
}

/// Whether the arm A run of pair `pair` is shown as attempt A. The order
/// is random but the same for the same `seed` and pair, so a judgment can
/// be repeated.
#[must_use]
pub fn a_first(seed: u64, pair: usize) -> bool {
    let pair = u64::try_from(pair).unwrap_or(u64::MAX);
    let mut rng = SplitMix64::seeded(seed ^ pair.wrapping_mul(PAIR_MIX));
    rng.unit() < 0.5
}

/// The request asking `model` to judge `a` against `b` on `task`, in the
/// order `a_first` says, against `reference` when there is one, sent with
/// the user's `auth_token` when the route needs one.
///
/// # Errors
/// Returns the builder's error when `model` is empty.
pub fn request(
    model: &str,
    task: &str,
    (a, b): (&Attempt, &Attempt),
    a_first: bool,
    reference: Option<&Attempt>,
    auth_token: Option<String>,
) -> Result<ModelRequest, ReasonerError> {
    let (first, second) = if a_first { (a, b) } else { (b, a) };
    let message = prompt(task, first, second, reference);
    Ask::new(ModelCallPurpose::Judge, SYSTEM_PROMPT, message).request(model, auth_token)
}
