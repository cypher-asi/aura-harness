//! A benchmark as the BENCH tab shows it, and how far it has got.

#[cfg(doc)]
use super::run_order;
use super::{
    BenchArm, BenchBaseline, BenchJudge, BenchLiveRun, BenchRun, BenchStatus, BenchTaskRow,
    BenchTaskSetup, BenchTokens,
};

/// A benchmark as the BENCH tab shows it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchView {
    /// Hex id of the benchmark's own agent
    pub id: String,
    /// Hex id of the agent at the top of the tree it was asked from
    pub root: String,
    /// `Compare #2`
    pub name: String,
    /// What it is about: its prompt, or a replayed session's first one
    pub title: String,
    pub started_ms: u64,
    /// `prompt`, `plan <file>`, `session "<title>"`, or `5 example tasks`
    pub source: String,
    pub status: BenchStatus,
    /// Runs per arm, or per example task and arm
    pub repeats: u32,
    /// The example tasks it ran, when its work is example tasks
    pub tasks: Vec<BenchTaskRow>,
    pub check: Option<String>,
    pub budget_usd: Option<f64>,
    /// Model that classified the runs' prompts; empty for benchmarks
    /// recorded before they named it
    pub classifier: String,
    /// The prompts each run got, in order
    pub prompts: Vec<String>,
    /// Arm A, then arm B
    pub arms: [BenchArm; 2],
    /// Runs that ended, in [`run_order`]
    pub runs: Vec<BenchRun>,
    /// Runs under way, in [`run_order`]; only while it runs
    pub live: Vec<BenchLiveRun>,
    pub judge: Option<BenchJudge>,
    /// The classifier's calls, made once per prompt for both arms and in
    /// neither arm's costs; empty until it ends
    pub classify: Vec<BenchTokens>,
    /// What a replayed session cost when it ran
    pub baseline: Option<BenchBaseline>,
    /// Whether the judge had what the user kept to measure against
    pub has_reference: bool,
    pub notes: Vec<String>,
}

/// What a benchmark is doing, as its progress shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchPhase {
    /// Fetching example tasks' files before any run works
    SettingUp,
    Running,
    /// Every run ended; the judge compares them
    Judging,
    Done,
    /// The user stopped it
    Stopped,
}

impl BenchPhase {
    /// `Setting up`, `Running`, ...
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SettingUp => "Setting up",
            Self::Running => "Running",
            Self::Judging => "Judging",
            Self::Done => "Done",
            Self::Stopped => "Stopped",
        }
    }

    /// Whether it is still going.
    #[must_use]
    pub const fn under_way(self) -> bool {
        matches!(self, Self::SettingUp | Self::Running | Self::Judging)
    }
}

/// How far a benchmark's runs have got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BenchProgress {
    /// Runs it makes: per arm, `repeats` of each example task, or
    /// `repeats` of its work
    pub total: usize,
    /// Runs that ended
    pub done: usize,
    /// Runs that ended in an error
    pub failed: usize,
    /// Runs working now, not waiting for their task's files
    pub working: usize,
    pub phase: BenchPhase,
}

impl BenchView {
    /// How far its runs have got and what it is doing.
    #[must_use]
    pub fn progress(&self) -> BenchProgress {
        let repeats = usize::try_from(self.repeats.max(1)).unwrap_or(usize::MAX);
        let done = self.runs.len();
        let total = repeats
            .saturating_mul(self.tasks.len().max(1))
            .saturating_mul(2)
            .max(done + self.live.len());
        let working = self.live.iter().filter(|r| r.wall_ms.is_some()).count();
        let fetching = self.tasks.iter().any(|t| {
            matches!(
                t.setup,
                BenchTaskSetup::Queued | BenchTaskSetup::Fetching { .. }
            )
        });
        let phase = match self.status {
            BenchStatus::Running if done == 0 && working == 0 && fetching => BenchPhase::SettingUp,
            BenchStatus::Running if done >= total && self.live.is_empty() => BenchPhase::Judging,
            BenchStatus::Running => BenchPhase::Running,
            BenchStatus::Cancelled => BenchPhase::Stopped,
            _ => BenchPhase::Done,
        };
        BenchProgress {
            total,
            done,
            failed: self.runs.iter().filter(|r| r.error.is_some()).count(),
            working,
            phase,
        }
    }
}
