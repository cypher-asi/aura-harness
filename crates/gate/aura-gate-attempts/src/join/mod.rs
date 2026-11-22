//! The lines of a log joined: each attempt once with the truth that
//! counts for it, and the reviews, each of the attempt with the same
//! agent and prompt.

use std::cell::OnceCell;
use std::collections::hash_map::{Entry, HashMap};

use aura_gate_evidence::{flags, Flag};

use crate::line::{Attempt, Line, Review, Truth};

/// The agent and prompt, both in hex, that an attempt and the truths and
/// reviews about it share.
type Key<'a> = (&'a str, &'a str);

/// An attempt as the log counts it, with the truth that counts for it.
#[derive(Debug)]
pub(crate) struct Counted<'a> {
    /// The attempt
    pub(crate) attempt: &'a Attempt,
    /// What showed its work right or wrong, when anything did
    pub(crate) truth: Option<&'a Truth>,
    /// The flags its evidence raises, read out when first asked for
    raised: OnceCell<Vec<Flag>>,
}

impl Counted<'_> {
    /// The flags the attempt's evidence raises.
    pub(crate) fn raised(&self) -> &[Flag] {
        self.raised
            .get_or_init(|| flags(&self.attempt.evidence, self.attempt.category))
    }
}

/// The lines of a log joined.
#[derive(Debug)]
pub(crate) struct Joined<'a> {
    /// Each attempt once, in the order it was first logged
    pub(crate) attempts: Vec<Counted<'a>>,
    /// Every review, oldest first
    pub(crate) reviews: Vec<&'a Review>,
    /// The place of each attempt in `attempts`
    at: HashMap<Key<'a>, usize>,
}

impl<'a> Joined<'a> {
    /// `lines` joined. An attempt logged more than once counts once: as
    /// it was last logged while the app ran (a follow-up that changes
    /// what Gate makes of an attempt logs it again), else as it was last
    /// rebuilt. Of several truths about one attempt, a task's own check
    /// outranks what the user did, and the later of equals counts.
    pub(crate) fn of(lines: &'a [Line]) -> Self {
        let mut read = Reading::default();
        for line in lines {
            let (agent, prompt, _) = line.key();
            match line {
                Line::Attempt(attempt) => read.attempt((agent, prompt), attempt),
                Line::Truth(truth) => read.truth((agent, prompt), truth),
                Line::Review(review) => read.reviews.push(review),
            }
        }
        read.joined()
    }

    /// The place in `attempts` of the attempt `review` is of, when the
    /// log holds it.
    pub(crate) fn attempt_of(&self, review: &Review) -> Option<usize> {
        let key = (review.agent.as_str(), review.prompt_tx.as_str());
        self.at.get(&key).copied()
    }
}

/// The lines of a log as far as they are read.
#[derive(Default)]
struct Reading<'a> {
    /// Each attempt once, as it counts so far
    attempts: Vec<&'a Attempt>,
    /// The place of each attempt in `attempts`
    at: HashMap<Key<'a>, usize>,
    /// The truth that counts so far about each attempt
    truths: HashMap<Key<'a>, &'a Truth>,
    /// Every review, oldest first
    reviews: Vec<&'a Review>,
}

impl<'a> Reading<'a> {
    fn attempt(&mut self, key: Key<'a>, attempt: &'a Attempt) {
        match self.at.entry(key) {
            Entry::Occupied(at) => {
                let held = &mut self.attempts[*at.get()];
                if !attempt.backfilled || held.backfilled {
                    *held = attempt;
                }
            }
            Entry::Vacant(at) => {
                at.insert(self.attempts.len());
                self.attempts.push(attempt);
            }
        }
    }

    fn truth(&mut self, key: Key<'a>, truth: &'a Truth) {
        match self.truths.entry(key) {
            Entry::Occupied(mut held) => {
                if held.get().source.rank() <= truth.source.rank() {
                    held.insert(truth);
                }
            }
            Entry::Vacant(held) => {
                held.insert(truth);
            }
        }
    }

    fn joined(self) -> Joined<'a> {
        let attempts = self
            .attempts
            .into_iter()
            .map(|attempt| {
                let key = (attempt.agent.as_str(), attempt.prompt_tx.as_str());
                Counted {
                    attempt,
                    truth: self.truths.get(&key).copied(),
                    raised: OnceCell::new(),
                }
            })
            .collect();
        Joined {
            attempts,
            reviews: self.reviews,
            at: self.at,
        }
    }
}

/// An attempt Gate let stand whose work was later shown right or wrong.
#[derive(Debug, Clone, Copy)]
pub struct Stood<'a> {
    /// The attempt, as [`evaluate_where`](crate::evaluate_where) counts it
    pub attempt: &'a Attempt,
    /// What showed its work right or wrong
    pub truth: &'a Truth,
    /// Attempts the log holds for the attempt's agent
    pub attempts_by_agent: usize,
}

/// The attempts in `lines` that Gate let stand and whose work was later
/// shown right or wrong, oldest first, each counted once as
/// [`evaluate_where`](crate::evaluate_where) counts it.
#[must_use]
pub fn stood(lines: &[Line]) -> Vec<Stood<'_>> {
    let joined = Joined::of(lines);
    let mut by_agent: HashMap<&str, usize> = HashMap::new();
    for counted in &joined.attempts {
        *by_agent.entry(&counted.attempt.agent).or_default() += 1;
    }
    joined
        .attempts
        .iter()
        .filter(|counted| counted.attempt.stood())
        .filter_map(|counted| {
            Some(Stood {
                attempt: counted.attempt,
                truth: counted.truth?,
                attempts_by_agent: by_agent[counted.attempt.agent.as_str()],
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
