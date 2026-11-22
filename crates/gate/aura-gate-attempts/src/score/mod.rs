//! Scoring Gate against what happened: how its verdicts held up, how
//! each sign of failure did on the attempts it let stand, and how the
//! reviews held up.

mod flags;
mod reviews;
mod verdicts;

pub use flags::{FlagScore, Scored};
pub use reviews::ReviewScore;
pub use verdicts::Verdicts;

use std::collections::BTreeMap;

use aura_gate_evidence::Flag;
use serde::Serialize;

use crate::join::{Counted, Joined};
use crate::line::{Attempt, Line, Truth};
use crate::tiers::Tiers;

/// What the log says of Gate's verdicts and of each [`Flag`].
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    /// Attempts in the log, counted once each
    pub attempts: u32,
    /// Attempts Gate chose the model of
    pub routed: u32,
    /// How Gate's verdicts held up, over every attempt
    pub verdicts: Verdicts,
    /// The same per model, over the attempts Gate let stand
    pub models: BTreeMap<String, Verdicts>,
    /// Attempts Gate let stand whose work was later shown right or wrong:
    /// what the flags are scored on
    pub stood: u32,
    /// Of those, the ones whose work was wrong
    pub stood_wrong: u32,
    /// Each flag's score, in [`Flag::ALL`]'s order
    pub flags: Vec<FlagScore>,
    /// The attempts the flags are scored on, oldest first
    pub scored: Vec<Scored>,
    /// What each flag has earned, over the whole log whatever the filter
    pub tiers: Tiers,
    /// How the reviews held up, per way of asking
    pub reviews: Vec<ReviewScore>,
}

impl Report {
    /// The failed attempts Gate let stand, oldest first.
    pub fn missed(&self) -> impl Iterator<Item = &Scored> {
        self.scored.iter().filter(|scored| !scored.right)
    }

    /// Count `counted`, an attempt the report is about.
    fn count(&mut self, counted: &Counted<'_>) {
        let attempt = counted.attempt;
        self.routed += u32::from(attempt.routed);
        let Some(truth) = counted.truth else {
            return;
        };
        self.verdicts.take(attempt.success, truth.passed);
        if !attempt.stood() {
            return;
        }
        self.models
            .entry(attempt.model.clone())
            .or_default()
            .take(attempt.success, truth.passed);
        self.stood += 1;
        self.stood_wrong += u32::from(!truth.passed);
        let raised = counted.raised();
        for score in &mut self.flags {
            score.count(raised, truth.passed);
        }
        self.scored
            .push(Scored::of(attempt, truth, raised.to_vec()));
    }
}

/// Which attempts a report is about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Only attempts Gate chose the model of
    pub routed: bool,
    /// Only attempts Gate counted as passed
    pub passed: bool,
    /// Only attempts at a benchmark task whose id contains this
    pub task: Option<String>,
}

impl Filter {
    fn takes(&self, attempt: &Attempt, truth: Option<&Truth>) -> bool {
        let task = self.task.as_deref().is_none_or(|wanted| {
            truth
                .and_then(|truth| truth.task.as_deref())
                .is_some_and(|task| task.contains(wanted))
        });
        task && (attempt.routed || !self.routed) && (attempt.success == Some(true) || !self.passed)
    }
}

/// Score Gate's verdicts and each flag on every attempt in `lines`; see
/// [`evaluate_where`].
#[must_use]
pub fn evaluate(lines: &[Line]) -> Report {
    evaluate_where(lines, &Filter::default())
}

/// Score Gate's verdicts, each flag and the reviews on the attempts in
/// `lines` that `filter` takes. An attempt logged more than once counts
/// once: as it was last logged while the app ran (a follow-up that
/// changes what Gate makes of an attempt logs it again), else as it was
/// last rebuilt. Of several truths about one attempt, a task's own check
/// outranks what the user did, and the later of equals counts. Of
/// several reviews of one attempt asked the same way, the last counts.
#[must_use]
pub fn evaluate_where(lines: &[Line], filter: &Filter) -> Report {
    let joined = Joined::of(lines);
    let taken: Vec<bool> = joined
        .attempts
        .iter()
        .map(|counted| filter.takes(counted.attempt, counted.truth))
        .collect();
    let mut report = Report {
        attempts: u32::try_from(taken.iter().filter(|&&taken| taken).count()).unwrap_or(u32::MAX),
        flags: Flag::ALL.into_iter().map(FlagScore::new).collect(),
        tiers: Tiers::earned(&joined),
        ..Report::default()
    };
    for (counted, &taken) in joined.attempts.iter().zip(&taken) {
        if taken {
            report.count(counted);
        }
    }
    report.reviews = reviews::scores(&joined, &taken);
    report
}

#[cfg(test)]
mod tests;
