//! Why a turn with no check looks wrong.

use aura_gate_evidence::transcript::reply::gave_up;
use aura_gate_evidence::{verifiable, Flag};

use crate::review::Why;
use crate::step::Ended;

/// Why a turn with no check looks wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suspicion {
    /// Work that changes code changed no file.
    NoChanges,
    /// Its reply says it could not do the work.
    GaveUp,
    /// It stopped on a limit.
    HitLimit,
    /// A check it ran passed, but its reply says the work was not
    /// verified.
    Unverified,
    /// A check it ran passed, but no test names what its change added.
    Uncovered,
}

impl Suspicion {
    /// The suspicion in words, for the judge.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::NoChanges => "the request asks for a code change, but no file was changed",
            Self::GaveUp => "the final reply says the work could not be done or was left unfinished",
            Self::HitLimit => "the attempt stopped on a limit (iterations, budget or a stall) instead of finishing",
            Self::Unverified => "a test or build it ran exited with success, but the final reply says the work was not verified or not tested",
            Self::Uncovered => "the tests it ran passed, but no test names anything its change added, so they never ran the new code",
        }
    }

    /// Why work it is raised on is reviewed: the sign of failure that
    /// says the same of an attempt's evidence, or [`Why::Uncovered`],
    /// which its evidence does not show.
    pub(crate) const fn why(self) -> Why {
        match self {
            Self::NoChanges => Why::Sign(Flag::NoChanges),
            Self::GaveUp => Why::Sign(Flag::GaveUp),
            Self::HitLimit => Why::Sign(Flag::HitLimit),
            Self::Unverified => Why::Sign(Flag::ReplyUnverified),
            Self::Uncovered => Why::Uncovered,
        }
    }
}

/// Why `ended`, a turn with no check, looks wrong; empty when nothing
/// does.
#[must_use]
pub fn suspicions(ended: &Ended<'_>) -> Vec<Suspicion> {
    let mut found = Vec::new();
    if verifiable(ended.category) && ended.files_changed == 0 {
        found.push(Suspicion::NoChanges);
    }
    if gave_up(ended.reply) {
        found.push(Suspicion::GaveUp);
    }
    if ended.hit_limit {
        found.push(Suspicion::HitLimit);
    }
    if !ended.uncovered.is_empty() {
        found.push(Suspicion::Uncovered);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The suspicions of unchecked work are told the judge as they are,
    /// so their words are pinned.
    #[test]
    fn the_suspicions_told_the_judge_are_pinned() {
        assert_eq!(
            [
                Suspicion::NoChanges,
                Suspicion::GaveUp,
                Suspicion::HitLimit,
                Suspicion::Unverified,
                Suspicion::Uncovered,
            ]
            .map(Suspicion::describe),
            [
                "the request asks for a code change, but no file was changed",
                "the final reply says the work could not be done or was left unfinished",
                "the attempt stopped on a limit (iterations, budget or a stall) instead of \
                 finishing",
                "a test or build it ran exited with success, but the final reply says the work \
                 was not verified or not tested",
                "the tests it ran passed, but no test names anything its change added, so they \
                 never ran the new code",
            ]
        );
    }

    #[test]
    fn a_suspicion_is_reviewed_as_the_sign_that_says_the_same() {
        assert_eq!(
            Suspicion::Unverified.why(),
            Why::Sign(Flag::ReplyUnverified)
        );
        assert_eq!(Suspicion::Uncovered.why().name(), "uncovered");
        for (suspicion, flag) in [
            (Suspicion::NoChanges, Flag::NoChanges),
            (Suspicion::GaveUp, Flag::GaveUp),
            (Suspicion::HitLimit, Flag::HitLimit),
        ] {
            assert_eq!(suspicion.why(), Why::Sign(flag));
            assert_eq!(suspicion.describe(), flag.describe(), "{suspicion:?}");
        }
    }
}
