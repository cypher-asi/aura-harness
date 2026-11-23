//! The next step for a turn Gate routed: how it ended, the steps already
//! taken for the attempt, and those the caller can take.

use aura_config::ReviewMode;
use aura_core_types::{Difficulty, OutcomeSignal, PromptCategory};
use aura_gate_evidence::transcript::reply::says_unverified;
use aura_gate_evidence::{verifiable, Flag};

use crate::review::{acted_review, reasons_in, review_decides, Why};
use crate::suspicion::{suspicions, Suspicion};

/// How a turn that Gate routed ended, as the steps need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ended<'a> {
    /// Its signal so far (see
    /// [`aura_gate_evidence::outcome::turn_signal`]); `None` while
    /// nothing has said how it went.
    pub signal: Option<OutcomeSignal>,
    pub category: PromptCategory,
    /// Files its tools changed.
    pub files_changed: usize,
    /// Its final reply.
    pub reply: &'a str,
    /// Whether it stopped on a limit (iterations, budget, a stall, a cut
    /// off reply) rather than by finishing.
    pub hit_limit: bool,
    /// How hard the classifier judged the work.
    pub difficulty: Option<Difficulty>,
    /// The signs its evidence raises that have earned a review of the
    /// attempts they are raised on (`aura_gate_attempts::Tier::Review`).
    pub review_signs: &'a [Flag],
    /// The signs its evidence raises that act on their own
    /// (`aura_gate_attempts::Tier::Act`).
    pub acting_signs: &'a [Flag],
    /// What its change added that no test in the workspace names, when
    /// its check passed; empty when a test names any of it, or when it
    /// is not known.
    pub uncovered: &'a [String],
    /// Whether it ran the code it changed itself after its last change,
    /// and that run succeeded ([`aura_gate_evidence::own_check`]).
    pub ran_own_check: bool,
    /// Whether its acceptance criteria were written down before it
    /// started.
    pub criteria: bool,
}

/// The steps already taken for the attempt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Taken {
    pub nudged: bool,
    pub checked: bool,
    pub judged: bool,
    /// Times it was reviewed
    pub reviews: u8,
    /// Whether the turn's work was sent back to its model with what was
    /// found, in this attempt or one before it
    pub repaired: bool,
}

/// The steps the caller can take.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Allowed {
    pub nudge: bool,
    /// A test command the harness can run is known.
    pub harness_check: bool,
    /// A model can judge the work.
    pub judge: bool,
    /// Whether work about to stand is reviewed; needs `judge`.
    pub review: ReviewMode,
    /// Whether work with acceptance criteria is reviewed against them;
    /// needs `judge`.
    pub criteria: ReviewMode,
}

/// What to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Nothing more: the turn stands as it ended.
    Stop,
    /// Ask the same model to verify its change: to test what it added
    /// that no test names, when these names are given.
    Nudge(Vec<String>),
    /// Run the project's tests.
    HarnessCheck,
    /// Have the judge review the work for these reasons.
    JudgeUnverified(Vec<Suspicion>),
    /// Have the judge go through the work against the request before it
    /// stands, for these reasons. Unless `act`, its verdict is only
    /// logged.
    Review { why: Vec<Why>, act: bool },
    /// Send the work back to the same model, told these signs.
    Repair(Vec<Flag>),
}

/// The next step for `ended`, given the steps `taken` and those
/// `allowed`. A turn with no signal yet is nudged and checked by the
/// harness when it changed code, may be reviewed, and is then judged:
/// whatever it asked for, it ends with a verdict. So is code work whose
/// check passed without a test naming what it added. Other code work
/// whose own check passed may be reviewed, and goes to the judge when its
/// reply says the work was not verified. Any other signal already says
/// how the turn went.
#[must_use]
pub fn next(ended: &Ended<'_>, taken: Taken, allowed: Allowed) -> Step {
    match ended.signal {
        None => unchecked(ended, taken, allowed),
        Some(OutcomeSignal::ChecksPassed) if verifiable(ended.category) => {
            if ended.uncovered.is_empty() {
                passed(ended, taken, allowed)
            } else {
                unchecked(ended, taken, allowed)
            }
        }
        Some(_) => Step::Stop,
    }
}

fn unchecked(ended: &Ended<'_>, taken: Taken, allowed: Allowed) -> Step {
    let code = verifiable(ended.category);
    let changed = code && ended.files_changed > 0;
    if changed && allowed.nudge && !taken.nudged && !ended.hit_limit && !ended.ran_own_check {
        return Step::Nudge(ended.uncovered.to_vec());
    }
    if changed && allowed.harness_check && !taken.checked {
        return Step::HarnessCheck;
    }
    if !allowed.judge {
        return Step::Stop;
    }
    let looks = suspicions(ended);
    if code {
        let why = reasons_in(ReviewMode::On, ended, &looks, allowed);
        if let Some(step) = acted_review(ended, taken, why, allowed) {
            return step;
        }
    }
    if taken.judged {
        return Step::Stop;
    }
    Step::JudgeUnverified(looks)
}

fn passed(ended: &Ended<'_>, taken: Taken, allowed: Allowed) -> Step {
    if !allowed.judge {
        return Step::Stop;
    }
    let looks = if !taken.checked && says_unverified(ended.reply) {
        vec![Suspicion::Unverified]
    } else {
        Vec::new()
    };
    let why = reasons_in(ReviewMode::On, ended, &looks, allowed);
    if let Some(step) = acted_review(ended, taken, why, allowed) {
        return step;
    }
    // The review stands in for the judge of work that looks wrong; when
    // it does not, such work goes to the judge below. The rest is
    // reviewed for the log.
    let decides = review_decides(ended, allowed);
    if (decides || looks.is_empty()) && taken.reviews == 0 {
        let why = reasons_in(ReviewMode::Log, ended, &[], allowed);
        if !why.is_empty() {
            return Step::Review { why, act: false };
        }
    }
    if decides {
        return Step::Stop;
    }
    if !taken.judged && !looks.is_empty() {
        return Step::JudgeUnverified(looks);
    }
    Step::Stop
}
