//! How the reviews held up against what happened, per way of asking and
//! reviewer.

use std::collections::BTreeMap;

use aura_gate_judge::turn::Answer;
use serde::Serialize;

use crate::join::Joined;
use crate::line::Review;

/// How the reviews one model was asked one way held up against what
/// happened.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewScore {
    /// How the judge was asked
    pub variant: String,
    /// The model that reviewed
    pub judge: String,
    /// Whether `aura gate replay` asked
    pub replayed: bool,
    /// Attempts reviewed
    pub reviewed: u32,
    /// Of those, the ones whose work was later shown right or wrong
    pub known: u32,
    /// Of the known, the ones whose work was wrong
    pub wrong: u32,
    /// Known attempts the judge found unfinished
    pub unfinished: u32,
    /// Of those, the ones whose work was wrong: failures it caught
    pub caught: u32,
    /// Of those, the ones whose work was right: good work it would send
    /// back
    pub false_alarms: u32,
    /// List-price cost of the reviews with a known price, in USD
    pub cost_usd: f64,
    /// Reviews whose price was unknown, which `cost_usd` leaves out
    #[serde(skip_serializing_if = "is_zero")]
    pub unpriced: u32,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde passes a reference
const fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl ReviewScore {
    /// The score of the reviews asked as `review` was, counting it, an
    /// attempt whose work was `right` when that is known.
    fn first(review: &Review, right: Option<bool>) -> Self {
        let mut score = Self {
            variant: review.variant.clone(),
            judge: review.judge.clone(),
            replayed: review.replayed,
            reviewed: 0,
            known: 0,
            wrong: 0,
            unfinished: 0,
            caught: 0,
            false_alarms: 0,
            cost_usd: 0.0,
            unpriced: 0,
        };
        score.count(review, right);
        score
    }

    /// Whether `review` was asked the way these reviews were.
    fn asked_as(&self, review: &Review) -> bool {
        self.variant == review.variant
            && self.judge == review.judge
            && self.replayed == review.replayed
    }

    /// Count `review`, of an attempt whose work was `right` when that is
    /// known.
    fn count(&mut self, review: &Review, right: Option<bool>) {
        self.reviewed += 1;
        match review.cost_usd {
            Some(cost_usd) => self.cost_usd += cost_usd,
            None => self.unpriced += 1,
        }
        let Some(right) = right else {
            return;
        };
        let unfinished = review.verdict == Answer::Model;
        self.known += 1;
        self.wrong += u32::from(!right);
        self.unfinished += u32::from(unfinished);
        self.caught += u32::from(unfinished && !right);
        self.false_alarms += u32::from(unfinished && right);
    }
}

/// How the reviews of the attempts of `joined` that `taken` marks held
/// up, per way of asking and reviewer, in that order. Of several reviews
/// of one attempt asked the same way by the same reviewer, the last
/// counts.
pub(super) fn scores(joined: &Joined<'_>, taken: &[bool]) -> Vec<ReviewScore> {
    let mut last = BTreeMap::new();
    for &review in &joined.reviews {
        let Some(at) = joined.attempt_of(review).filter(|&at| taken[at]) else {
            continue;
        };
        let asked = (
            review.variant.as_str(),
            review.judge.as_str(),
            review.replayed,
        );
        last.insert(
            (asked, review.agent.as_str(), review.prompt_tx.as_str()),
            (review, at),
        );
    }
    let mut scores: Vec<ReviewScore> = Vec::new();
    for (review, at) in last.into_values() {
        let right = joined.attempts[at].truth.map(|truth| truth.passed);
        match scores.last_mut() {
            Some(score) if score.asked_as(review) => score.count(review, right),
            _ => scores.push(ReviewScore::first(review, right)),
        }
    }
    scores
}
