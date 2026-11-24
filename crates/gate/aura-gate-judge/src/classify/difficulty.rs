//! How hard work is, worked out from what the classifier found about it
//! rather than asked for outright: a model asked for a difficulty calls
//! nearly every real change hard, while what it finds (how much code
//! changes, how precisely it is asked for, how much must be read first,
//! the traps in it) it reports far more steadily.
//!
//! Each finding scores points; the total picks the difficulty. The
//! weights are a starting point, to be tuned against which tier of model
//! actually passed each task.

use aura_core_types::{Assessment, Difficulty, Exploration, Scope, Spec};

/// The most points work scores and is still easy.
pub const EASY_AT_MOST: u32 = 2;

/// The fewest points work scores to be hard.
pub const HARD_AT_LEAST: u32 = 6;

/// How hard the work `assessment` describes is: easy at
/// [`EASY_AT_MOST`] points or fewer, hard at [`HARD_AT_LEAST`] or more,
/// medium between.
#[must_use]
pub fn difficulty_of(assessment: &Assessment) -> Difficulty {
    match points(assessment) {
        p if p <= EASY_AT_MOST => Difficulty::Easy,
        p if p >= HARD_AT_LEAST => Difficulty::Hard,
        _ => Difficulty::Medium,
    }
}

/// The points `assessment` scores: up to 3 for scope, 2 for a loose spec,
/// 2 for deep exploring and 3 for traps (2 for the first, 1 for any
/// more), less 1 when the request says how to check the result.
#[must_use]
pub fn points(assessment: &Assessment) -> u32 {
    let scope = match assessment.scope {
        Scope::OneSpot => 0,
        Scope::OneFile => 1,
        Scope::FewFiles => 2,
        Scope::ManyFiles => 3,
    };
    let spec = match assessment.spec {
        Spec::Exact => 0,
        Spec::Clear => 1,
        Spec::Loose => 2,
    };
    let exploration = match assessment.exploration {
        Exploration::None => 0,
        Exploration::Some => 1,
        Exploration::Deep => 2,
    };
    let traps = match assessment.pitfalls.len() {
        0 => 0,
        1 => 2,
        _ => 3,
    };
    let total: u32 = scope + spec + exploration + traps;
    total.saturating_sub(u32::from(assessment.checkable))
}
