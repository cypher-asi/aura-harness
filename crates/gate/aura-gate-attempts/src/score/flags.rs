//! How each sign of failure did on the attempts Gate let stand whose
//! work was later shown right or wrong, and those attempts.

use aura_core_types::OutcomeSignal;
use aura_gate_evidence::Flag;
use serde::Serialize;

use crate::line::{Attempt, Truth, TruthSource};

/// How a [`Flag`] did on the attempts Gate let stand whose work was
/// later shown right or wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FlagScore {
    /// The flag
    pub flag: Flag,
    /// Attempts it was raised on
    pub raised: u32,
    /// Of those, the ones whose work was wrong: failures it would have
    /// caught
    pub caught: u32,
    /// Of those, the ones whose work was right: good attempts it would
    /// have questioned
    pub false_alarms: u32,
}

impl FlagScore {
    /// The score of `flag` before any attempt is counted.
    pub(super) const fn new(flag: Flag) -> Self {
        Self {
            flag,
            raised: 0,
            caught: 0,
            false_alarms: 0,
        }
    }

    /// Count an attempt that raised `raised` and whose work was `right`.
    pub(super) fn count(&mut self, raised: &[Flag], right: bool) {
        if raised.contains(&self.flag) {
            self.raised += 1;
            self.caught += u32::from(!right);
            self.false_alarms += u32::from(right);
        }
    }
}

/// An attempt Gate let stand whose work was later shown right or wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Scored {
    /// The comparison and run it belongs to, when it has one, e.g.
    /// `Compare #22 a-4`
    pub run: String,
    /// The benchmark task it did, when it did one
    pub task: String,
    /// The model that did the work
    pub model: String,
    /// Whether Gate chose the model
    pub routed: bool,
    /// What Gate recorded of how it went
    pub signal: OutcomeSignal,
    /// Whether the work was right
    pub right: bool,
    /// What said so
    pub source: TruthSource,
    /// The flags its evidence raises
    pub flags: Vec<Flag>,
}

impl Scored {
    /// `attempt`, which raised `flags`, as `truth` proved it.
    pub(super) fn of(attempt: &Attempt, truth: &Truth, flags: Vec<Flag>) -> Self {
        let run = [truth.compare.as_deref(), truth.run.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        Self {
            run,
            task: truth.task.clone().unwrap_or_default(),
            model: attempt.model.clone(),
            routed: attempt.routed,
            signal: attempt.signal,
            right: truth.passed,
            source: truth.source,
            flags,
        }
    }
}
