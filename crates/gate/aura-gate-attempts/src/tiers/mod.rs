//! What each sign of failure has earned on the attempts it was raised
//! on: nothing, a review of those attempts, or sending them straight
//! back to the model.

use aura_core_types::Difficulty;
use aura_gate_evidence::Flag;
use aura_gate_judge::turn::Answer;
use aura_model::model_catalog::{infer_tier, ModelTier};
use serde::{Deserialize, Serialize};

use crate::join::Joined;
use crate::line::{Line, Review};

/// What a [`Flag`] has earned on the attempts it was raised on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// It is logged and scored, nothing more.
    #[default]
    Log,
    /// An attempt it is raised on is reviewed before it stands.
    Review,
    /// An attempt it is raised on goes straight back to the model.
    Act,
}

impl Tier {
    /// The tier's name, e.g. `review`.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Log => "log only",
            Self::Review => "review",
            Self::Act => "acts",
        }
    }
}

/// Raises a flag needs before it can call for a review.
pub(crate) const REVIEW_MIN_RAISED: u32 = 5;

/// Wrong attempts among those a flag was raised on that it needs before
/// it can call for a review.
const REVIEW_MIN_WRONG: u32 = 2;

/// How many times as often as attempts of the same difficulty the
/// attempts a flag is raised on must be wrong for it to call for a
/// review.
pub(crate) const REVIEW_LIFT: f64 = 2.0;

/// Raises a flag needs before it can act on its own.
pub(crate) const ACT_MIN_RAISED: u32 = 10;

/// Share of the attempts a flag is raised on that must be wrong for it
/// to act on its own.
const ACT_PRECISION: f64 = 0.8;

/// Slack that keeps a record which meets a bar exactly from missing it
/// by rounding: what chance expects is a sum of shares.
const SLACK: f64 = 1e-9;

/// A [`Flag`]'s record on the attempts [`tiers`] counts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SignRecord {
    /// The flag
    pub flag: Flag,
    /// Attempts it was raised on
    pub raised: u32,
    /// Of those, the ones whose work was wrong
    pub wrong: u32,
    /// How many of them would be wrong if the flag said nothing: the sum
    /// over them of the share of attempts of the same difficulty that
    /// were wrong
    pub expected: f64,
    /// What that earns it
    pub tier: Tier,
}

impl SignRecord {
    /// The record of `flag` on the `counted` attempts, where `rates`
    /// holds the share of wrong attempts among those not hard, then
    /// among the hard ones.
    fn of(flag: Flag, counted: &[Known<'_>], rates: [f64; 2]) -> Self {
        let (mut raised, mut wrong, mut expected) = (0, 0, 0.0);
        for attempt in counted
            .iter()
            .filter(|attempt| attempt.raised.contains(&flag))
        {
            raised += 1;
            wrong += u32::from(attempt.wrong);
            expected += rates[usize::from(attempt.hard)];
        }
        Self {
            flag,
            raised,
            wrong,
            expected,
            tier: Self::tier_of(raised, wrong, expected),
        }
    }

    fn tier_of(raised: u32, wrong: u32, expected: f64) -> Tier {
        let (raised_f, wrong_f) = (f64::from(raised), f64::from(wrong) + SLACK);
        if raised >= ACT_MIN_RAISED && wrong_f >= ACT_PRECISION * raised_f {
            Tier::Act
        } else if raised >= REVIEW_MIN_RAISED
            && wrong >= REVIEW_MIN_WRONG
            && wrong_f >= REVIEW_LIFT * expected
        {
            Tier::Review
        } else {
            Tier::Log
        }
    }

    /// What the flag lacks for the next tier, in words.
    pub(crate) fn lacks(&self) -> String {
        match self.tier {
            Tier::Act => String::new(),
            Tier::Review => format!("acts at {ACT_MIN_RAISED} raises with 4 in 5 wrong"),
            Tier::Log if self.raised < REVIEW_MIN_RAISED => {
                format!("{} more raises", REVIEW_MIN_RAISED - self.raised)
            }
            Tier::Log => format!(
                "wrong on {:.0} of its raises",
                (REVIEW_LIFT * self.expected - SLACK)
                    .ceil()
                    .max(f64::from(REVIEW_MIN_WRONG))
            ),
        }
    }
}

/// What each [`Flag`] has earned; see [`tiers`].
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Tiers {
    /// Attempts counted
    pub known: u32,
    /// Of those, the ones whose work was wrong
    pub wrong: u32,
    /// Each flag's record, in [`Flag::ALL`]'s order
    pub signs: Vec<SignRecord>,
}

impl Tiers {
    /// The tier of `flag`.
    #[must_use]
    pub fn of(&self, flag: Flag) -> Tier {
        self.signs
            .iter()
            .find(|sign| sign.flag == flag)
            .map_or(Tier::Log, |sign| sign.tier)
    }

    /// Those of `flags` in `tier`.
    #[must_use]
    pub fn in_tier(&self, flags: &[Flag], tier: Tier) -> Vec<Flag> {
        flags
            .iter()
            .copied()
            .filter(|flag| self.of(*flag) == tier)
            .collect()
    }

    /// What each flag has earned on the attempts of `joined`; see
    /// [`tiers`].
    pub(crate) fn earned(joined: &Joined<'_>) -> Self {
        let counted = known(joined);
        let rates = [wrong_rate(&counted, false), wrong_rate(&counted, true)];
        Self {
            known: u32::try_from(counted.len()).unwrap_or(u32::MAX),
            wrong: u32::try_from(counted.iter().filter(|attempt| attempt.wrong).count())
                .unwrap_or(u32::MAX),
            signs: Flag::ALL
                .into_iter()
                .map(|flag| SignRecord::of(flag, &counted, rates))
                .collect(),
        }
    }
}

/// Whether `model` is below the strongest tier, where Gate's follow-up
/// applies: work there has a stronger model to go on to.
#[must_use]
pub fn below_strongest(model: &str) -> bool {
    infer_tier(model) < ModelTier::Frontier
}

/// What each [`Flag`] has earned on the attempts in `lines` that Gate
/// routed to a model below the strongest tier and whose work is known
/// right or wrong: those it let stand that something later proved, and
/// those a review it acted on found unfinished, which count as wrong
/// with the signs they showed then.
///
/// A flag calls for a review ([`Tier::Review`]) once it was raised
/// `REVIEW_MIN_RAISED` times and the attempts it was raised on were
/// wrong `REVIEW_LIFT` times as often as attempts of the same difficulty
/// (hard, or not) are. It acts on its own ([`Tier::Act`]) once it was
/// raised `ACT_MIN_RAISED` times and `ACT_PRECISION` of those attempts
/// were wrong.
#[must_use]
pub fn tiers(lines: &[Line]) -> Tiers {
    Tiers::earned(&Joined::of(lines))
}

/// An attempt [`tiers`] counts, whose work is known right or wrong.
struct Known<'a> {
    /// Whether the classifier judged it hard
    hard: bool,
    /// Whether its work was wrong
    wrong: bool,
    /// The flags it raised
    raised: &'a [Flag],
}

/// The attempts of `joined` that [`tiers`] counts, in order.
fn known<'a>(joined: &'a Joined<'_>) -> Vec<Known<'a>> {
    // The first review Gate acted on that found each attempt unfinished.
    let mut sent_back: Vec<Option<&Review>> = vec![None; joined.attempts.len()];
    let acted_on = joined
        .reviews
        .iter()
        .copied()
        .filter(|review| review.acted && !review.replayed && review.verdict == Answer::Model);
    for review in acted_on {
        if let Some(at) = joined.attempt_of(review) {
            sent_back[at].get_or_insert(review);
        }
    }
    joined
        .attempts
        .iter()
        .zip(sent_back)
        .filter(|(counted, _)| counted.attempt.routed && below_strongest(&counted.attempt.model))
        .filter_map(|(counted, sent_back)| {
            let hard = counted.attempt.difficulty == Some(Difficulty::Hard);
            if let Some(review) = sent_back {
                return Some(Known {
                    hard,
                    wrong: true,
                    raised: &review.signs,
                });
            }
            let truth = counted.truth.filter(|_| counted.attempt.stood())?;
            Some(Known {
                hard,
                wrong: !truth.passed,
                raised: counted.raised(),
            })
        })
        .collect()
}

/// The share of the `counted` attempts that are hard, or are not, as
/// `hard` says, whose work was wrong; 0 when there are none.
fn wrong_rate(counted: &[Known<'_>], hard: bool) -> f64 {
    let (known, wrong) = counted
        .iter()
        .filter(|attempt| attempt.hard == hard)
        .fold((0u32, 0u32), |(known, wrong), attempt| {
            (known + 1, wrong + u32::from(attempt.wrong))
        });
    if known == 0 {
        0.0
    } else {
        f64::from(wrong) / f64::from(known)
    }
}

#[cfg(test)]
mod tests;
