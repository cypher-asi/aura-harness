//! The steps [`next`] gives turns that ended in each way, by topic.

use aura_config::ReviewMode;
use aura_core_types::{Difficulty, OutcomeSignal, PromptCategory};
use aura_gate_evidence::Flag;

use super::*;

mod criteria;
mod review;
mod unchecked;

/// Every step allowed, reviews off.
const ALL: Allowed = Allowed {
    nudge: true,
    harness_check: true,
    judge: true,
    review: ReviewMode::Off,
    criteria: ReviewMode::Off,
};

/// Every step allowed, reviews on.
const REVIEWING: Allowed = Allowed {
    review: ReviewMode::On,
    ..ALL
};

/// Every step allowed, reviews only logged.
const LOGGING: Allowed = Allowed {
    review: ReviewMode::Log,
    ..ALL
};

/// Code work with no signal yet that changed `files_changed` files and
/// replied `reply`.
fn ended(files_changed: usize, reply: &str) -> Ended<'_> {
    Ended {
        signal: None,
        category: PromptCategory::MultiFileChange,
        files_changed,
        reply,
        hit_limit: false,
        difficulty: None,
        review_signs: &[],
        acting_signs: &[],
        uncovered: &[],
        ran_own_check: false,
        criteria: false,
    }
}

/// Code work whose check passed, which replied `reply`.
fn passed(reply: &str) -> Ended<'_> {
    Ended {
        signal: Some(OutcomeSignal::ChecksPassed),
        ..ended(2, reply)
    }
}

fn review(why: &[Why], act: bool) -> Step {
    Step::Review {
        why: why.to_vec(),
        act,
    }
}
