//! The agents panel's BENCH tab: comparisons of two arms (each Gate or a
//! pinned model) on the same work, what they cost, and how well they did.
//!
//! The work is one prompt, the user's last prompt, or an earlier session
//! replayed prompt by prompt. Benchmarks are `/compare` comparisons; BENCH
//! lists them, sets new ones up and shows each one's numbers.

mod cost;
mod judge;
mod runs;
mod setup;
mod tasks;
#[cfg(test)]
mod tests;
mod view;

pub use cost::{
    cost_per_fix, cost_shares, per_fix_cells, unpriced_note, BenchBaseline, BenchCost,
    UNPRICED_MARK,
};
pub use judge::{BenchJudge, BenchVerdict};
pub use runs::{BenchArm, BenchDoing, BenchLiveRun, BenchRun, BenchTokens, BenchTurn};
pub use setup::{BenchExample, BenchRequest, BenchSession, BenchSpec, BenchWork};
pub use tasks::{BenchTaskCell, BenchTaskRow, BenchTaskSetup};
pub use view::{BenchPhase, BenchProgress, BenchView};

/// Where a benchmark stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BenchStatus {
    /// Runs are under way.
    #[default]
    Running,
    /// Every run ended; the copies are kept.
    Done,
    /// The user stopped it.
    Cancelled,
    /// The named run's changes went into the user's workspace.
    Applied(String),
    /// The copies were removed.
    Dismissed,
}

impl BenchStatus {
    /// `running`, `done`, `applied a-2`, ...
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Running => "running".to_string(),
            Self::Done => "done".to_string(),
            Self::Cancelled => "stopped".to_string(),
            Self::Applied(run) => format!("applied {run}"),
            Self::Dismissed => "dismissed".to_string(),
        }
    }

    /// Whether the runs' copies still exist, to diff or apply.
    #[must_use]
    pub const fn has_copies(&self) -> bool {
        !matches!(self, Self::Applied(_) | Self::Dismissed)
    }
}

/// `A` for arm 0, `B` for arm 1.
#[must_use]
pub const fn arm_letter(arm: usize) -> &'static str {
    if arm == 0 {
        "A"
    } else {
        "B"
    }
}

/// Where run `name` (`a-1`, `b-2`) of arm `arm` goes among a benchmark's
/// runs: by its number, then A before B. A name with no number goes last.
#[must_use]
pub fn run_order(name: &str, arm: usize) -> (u32, usize) {
    let number = name
        .rsplit_once('-')
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or(u32::MAX);
    (number, arm)
}
