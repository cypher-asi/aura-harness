//! Why work about to stand is reviewed, and when a review is due.

use aura_config::ReviewMode;
use aura_core_types::Difficulty;
use aura_gate_evidence::Flag;

use crate::step::{Allowed, Ended, Step, Taken};
use crate::suspicion::Suspicion;

/// Why work about to stand is reviewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// The work was classified hard.
    Hard,
    /// Its evidence raises this sign.
    Sign(Flag),
    /// Its check passed, but no test names what its change added.
    Uncovered,
    /// Its acceptance criteria were written down before it started.
    Criteria,
}

impl Why {
    /// The reason's name as logged: `hard`, `uncovered`, `criteria`, or
    /// the sign's.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Hard => "hard",
            Self::Sign(flag) => flag.name(),
            Self::Uncovered => "uncovered",
            Self::Criteria => "criteria",
        }
    }

    /// The reason in words, for the reviewer.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Hard => {
                "the task was rated hard, and hard work is reviewed before it is accepted"
            }
            Self::Sign(flag) => flag.describe(),
            Self::Uncovered => Suspicion::Uncovered.describe(),
            Self::Criteria => {
                "acceptance criteria were written down before the work started, and the work is \
                 checked against each before it is accepted"
            }
        }
    }
}

/// Why `ended` is reviewed before it stands, for being hard, showing a
/// sign or looking wrong (`looks`); empty when nothing calls for it.
fn reasons(ended: &Ended<'_>, looks: &[Suspicion]) -> Vec<Why> {
    let mut why = Vec::new();
    if ended.difficulty == Some(Difficulty::Hard) {
        why.push(Why::Hard);
    }
    let signs = ended
        .acting_signs
        .iter()
        .chain(ended.review_signs)
        .map(|sign| Why::Sign(*sign))
        .chain(looks.iter().map(|suspicion| suspicion.why()));
    for sign in signs {
        if !why.contains(&sign) {
            why.push(sign);
        }
    }
    why
}

/// Why `ended` gets a review in `mode`: [`reasons`] when reviews are in
/// `mode`, and its criteria when reviews against them are. A review in
/// [`ReviewMode::On`] is acted on; one in [`ReviewMode::Log`] is only
/// logged.
pub(crate) fn reasons_in(
    mode: ReviewMode,
    ended: &Ended<'_>,
    looks: &[Suspicion],
    allowed: Allowed,
) -> Vec<Why> {
    let mut why = if allowed.review == mode {
        reasons(ended, looks)
    } else {
        Vec::new()
    };
    if ended.criteria && allowed.criteria == mode {
        why.push(Why::Criteria);
    }
    why
}

/// Whether a review of `ended` stands in for the judge's verdict.
pub(crate) fn review_decides(ended: &Ended<'_>, allowed: Allowed) -> bool {
    allowed.review == ReviewMode::On || (ended.criteria && allowed.criteria == ReviewMode::On)
}

/// The acted review or repair `ended` is due for `why`, if any: twice at
/// most, and the second time only after the work was sent back. With
/// [`ReviewMode::On`], a sign that acts on its own sends the work back
/// first.
pub(crate) fn acted_review(
    ended: &Ended<'_>,
    taken: Taken,
    why: Vec<Why>,
    allowed: Allowed,
) -> Option<Step> {
    let fresh = taken.reviews == 0 && !taken.repaired;
    if allowed.review == ReviewMode::On
        && fresh
        && !ended.hit_limit
        && !ended.acting_signs.is_empty()
    {
        return Some(Step::Repair(ended.acting_signs.to_vec()));
    }
    let due = taken.reviews == 0 || (taken.reviews == 1 && taken.repaired);
    (due && !why.is_empty()).then_some(Step::Review { why, act: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reasons for a review are told the reviewer as they are, so
    /// their words are pinned.
    #[test]
    fn the_reasons_given_the_reviewer_are_pinned() {
        let why = [
            Why::Hard,
            Why::Sign(Flag::NoCheck),
            Why::Uncovered,
            Why::Criteria,
        ];
        assert_eq!(
            why.map(Why::name),
            ["hard", "no_check", "uncovered", "criteria"]
        );
        assert_eq!(
            why.map(Why::describe),
            [
                "the task was rated hard, and hard work is reviewed before it is accepted",
                "no test or build was run to its end",
                "the tests it ran passed, but no test names anything its change added, so they \
                 never ran the new code",
                "acceptance criteria were written down before the work started, and the work is \
                 checked against each before it is accepted",
            ]
        );
        assert_eq!(Why::Sign(Flag::ShellEdit).name(), "shell_edit");
    }
}
