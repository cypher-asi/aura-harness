//! Layer: gate
//!
//! What Gate does about a turn it routed that ended with nothing to judge
//! it by: no test or build of its own, and no error; or with a check
//! that passed while its own reply says the work was not verified.
//!
//! Every routed turn ends with a verdict. One with no check gets it here,
//! cheapest first:
//!
//! 1. [`Step::Nudge`]: code work that changed files is asked once, on the
//!    same model, to run the tests that cover its change. Its own check
//!    then says how it went. Work that already ran the changed code
//!    itself ([`aura_gate_evidence::own_check`]), as the nudge asks where
//!    no test covers it, is not asked.
//! 2. [`Step::HarnessCheck`]: when it still ran none, the harness runs
//!    the project's tests itself.
//! 3. [`Step::JudgeUnverified`]: when nothing could be run, the judge
//!    reviews the work without a check and its verdict is the outcome,
//!    whatever the work asked for. What looks wrong ([`suspicions`]) is
//!    pointed out to it.
//!
//! A turn whose check passed while no test in the workspace names
//! anything its change added ([`aura_gate_evidence::coverage`]) checked
//! none of it, and takes the steps above as a turn with no check does:
//! its nudge names what no test calls ([`uncovered_prompt`]), and the
//! judge is told why it looks.
//!
//! A turn whose check passed is not thereby right: the check may cover
//! little of what was asked. On the benchmark runs in Gate's attempt log
//! (`aura-gate-attempts`), a fifth of the hard attempts by models below
//! the strongest tier that Gate let stand had failed the task's own
//! check, most of them after a check of their own had passed. So before
//! such work stands it may be reviewed:
//!
//! 4. [`Step::Review`]: the judge goes through the request against
//!    everything the attempt changed. It is asked when the work was
//!    classified hard, when its evidence raises a sign the attempt log
//!    backs (`aura_gate_attempts::tiers`), or when it looks wrong as in
//!    step 3 or its reply says the work was not verified
//!    ([`says_unverified`](aura_gate_evidence::transcript::reply::says_unverified)).
//!    Work it finds unfinished goes back to the same model once with the
//!    findings ([`repair_prompt`]) and is reviewed again; found
//!    unfinished a second time, it fails. A turn is sent back once: work
//!    that has moved on to a stronger model and is found unfinished
//!    there fails at once.
//! 5. [`Step::Repair`]: a sign that has proven itself outright in the
//!    log skips the review and sends the work back at once.
//!
//! The review is governed by [`ReviewMode`](aura_config::ReviewMode):
//! `on` as above, and for work with no check its verdict stands in for
//! the judge's; `off` does not ask, which leaves steps 1 to 3 and the one
//! case the judge always took, a passed check the reply disowns; `log` is
//! `off` plus a review whose verdict is only logged, of passed work the
//! judge would not see.
//!
//! Code work whose acceptance criteria were written down before it
//! started (`aura_gate_judge::criteria`) is also reviewed against them,
//! as its own review mode says: with `on`, every such attempt is reviewed
//! as above and its review stands in for the judge's; with `log`, work
//! about to stand gets a logged review against them if it gets no other
//! review.
//!
//! A failure any of them turns up goes on to a stronger model as a failed
//! check or a judged failure does. Each step runs at most once per
//! attempt, the review twice when the work was sent back between. Only
//! the judge's verdict is asked on the strongest tier, where there is no
//! stronger model to go on to; the other steps are offered only below it.
//!
//! - `step`: the next [`Step`] for a turn, from how it [`Ended`]
//! - `review`: [`Why`] work about to stand is reviewed, and when
//! - `suspicion`: why a turn with no check looks wrong ([`Suspicion`])
//! - `prompts`: what the model is told when its work is sent back, or
//!   goes on to a stronger model ([`takeover_prompt`], [`retry_prompt`])

mod prompts;
mod review;
mod step;
mod suspicion;
#[cfg(test)]
mod tests;

pub use prompts::{
    repair_prompt, retry_prompt, sign_repair_prompt, takeover_prompt, uncovered_prompt,
    NUDGE_PROMPT, REPAIR_OPENING,
};
pub use review::Why;
pub use step::{next, Allowed, Ended, Step, Taken};
pub use suspicion::{suspicions, Suspicion};
