//! Arms and their runs: tokens, per-prompt turns, ended and live runs.

use super::BenchCost;

/// Tokens and cost one model used across an arm's runs, for the runs'
/// own work or for one purpose beside it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchTokens {
    pub model: String,
    /// What the calls were made for beside the runs' own work
    /// (`classify`, `criteria`, `judge`, `title`); `None` for the work
    pub purpose: Option<String>,
    pub calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// What its priced calls cost
    pub cost_usd: f64,
    /// Its calls with no known price, which `cost_usd` leaves out
    pub unpriced_calls: u32,
}

impl BenchTokens {
    /// What its calls cost, as far as they had a price.
    #[must_use]
    pub const fn cost(&self) -> BenchCost {
        BenchCost::new(self.cost_usd, self.unpriced_calls)
    }
}

/// How an arm's runs did on one prompt.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchTurn {
    /// Runs that got to the prompt
    pub runs: usize,
    /// Mean cost of their turns
    pub mean_cost_usd: Option<f64>,
    /// Calls of their turns with no known price, which `mean_cost_usd`
    /// leaves out
    pub unpriced_calls: u32,
    /// Turns whose last test or build command passed
    pub passed: usize,
    /// Turns whose last test or build command failed
    pub failed: usize,
    /// Turns that ended in an error
    pub errors: usize,
}

impl BenchTurn {
    /// Mean cost of its turns, as far as their calls had a price.
    #[must_use]
    pub fn mean_cost(&self) -> Option<BenchCost> {
        self.mean_cost_usd
            .map(|usd| BenchCost::new(usd, self.unpriced_calls))
    }
}

/// One arm of a benchmark.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchArm {
    /// What it runs on: `gate`, `gate:nodelegate`, or a model
    pub label: String,
    /// Runs that started
    pub runs: usize,
    /// Runs that ended without an error
    pub finished: usize,
    /// What the priced calls of every run cost
    pub total_cost_usd: f64,
    /// Median cost of the finished runs
    pub median_cost_usd: Option<f64>,
    /// Calls of every run with no known price, which `total_cost_usd`
    /// leaves out
    pub unpriced_calls: u32,
    /// Calls of the finished runs with no known price, which
    /// `median_cost_usd` leaves out
    pub finished_unpriced_calls: u32,
    pub median_wall_ms: Option<u64>,
    pub checks_passed: usize,
    pub checks_run: usize,
    pub judge_wins: usize,
    pub judge_losses: usize,
    pub judge_ties: usize,
    /// Runs the judge rated better than, as good as, or worse than what
    /// the user kept from a replayed session
    pub ref_better: usize,
    pub ref_as_good: usize,
    pub ref_worse: usize,
    /// Tokens per model and purpose, most expensive first
    pub tokens: Vec<BenchTokens>,
    /// One per prompt
    pub turns: Vec<BenchTurn>,
    /// Runs a failed attempt moved to a stronger model
    pub escalated: usize,
    /// Runs Gate asked to check a change they had not checked
    pub nudged: usize,
}

impl BenchArm {
    /// What every run cost, as far as its calls had a price.
    #[must_use]
    pub const fn total_cost(&self) -> BenchCost {
        BenchCost::new(self.total_cost_usd, self.unpriced_calls)
    }

    /// Median cost of the finished runs, as far as their calls had a
    /// price.
    #[must_use]
    pub fn median_cost(&self) -> Option<BenchCost> {
        self.median_cost_usd
            .map(|usd| BenchCost::new(usd, self.finished_unpriced_calls))
    }
}

/// One run of a benchmark.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchRun {
    /// `a-1`, `b-2`, ...
    pub name: String,
    /// 0 for arm A, 1 for arm B
    pub arm: usize,
    /// Model its first turn was sent to
    pub model: String,
    /// The stronger model a failed attempt last moved its work to, or
    /// the same model when the move only raised its effort
    pub escalated_to: Option<String>,
    /// The effort that move raised it to (`max`), when it raised one
    pub escalated_effort: Option<String>,
    /// What the classifier said its first prompt asks for, as stored
    /// (`multi_file_change`); `None` in older records
    pub category: Option<String>,
    /// How hard the classifier said its first prompt's work is (`hard`)
    pub difficulty: Option<String>,
    /// What the classifier found drove that difficulty (`many files,
    /// loose spec, deep exploring`); `None` when it named the difficulty
    /// itself, and in older records
    pub drivers: Option<String>,
    /// Whether Gate had its model lead a turn, handing the work to
    /// subtasks
    pub led: bool,
    /// The part of `cost_usd` its subtasks spent
    pub subtask_cost_usd: f64,
    pub wall_ms: u64,
    /// What its priced calls cost
    pub cost_usd: f64,
    /// Its calls with no known price, which `cost_usd` leaves out
    pub unpriced_calls: u32,
    pub check_passed: Option<bool>,
    pub files_changed: u32,
    pub error: Option<String>,
    /// Id of the example task it worked on, in a benchmark of them
    pub task: Option<String>,
    /// Hex id of the agent it ran as, whose LOG tab shows its trace
    pub agent_id: String,
}

impl BenchRun {
    /// What it cost, as far as its calls had a price.
    #[must_use]
    pub const fn cost(&self) -> BenchCost {
        BenchCost::new(self.cost_usd, self.unpriced_calls)
    }
}

/// A run that has started and not ended.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchLiveRun {
    /// `a-1`, ...
    pub name: String,
    /// 0 for A, 1 for B
    pub arm: usize,
    /// The example task it works on
    pub task: Option<String>,
    /// The model its prompt under way went to; `None` until it was routed
    pub model: Option<String>,
    /// What the classifier said that prompt asks for, as stored
    /// (`multi_file_change`), and how hard (`hard`)
    pub category: Option<String>,
    pub difficulty: Option<String>,
    /// How long it has worked; `None` while it waits for its task's files
    pub wall_ms: Option<u64>,
    /// What its priced model calls have cost so far
    pub cost_usd: f64,
    /// Its calls so far with no known price, which `cost_usd` leaves out
    pub unpriced_calls: u32,
    /// Tokens its calls have used so far, per model and purpose
    pub tokens: Vec<BenchTokens>,
    /// Hex id of the agent it runs as; `None` while it waits for its
    /// task's files
    pub agent_id: Option<String>,
    /// What it is doing right now; `None` when that is not known
    pub doing: Option<BenchDoing>,
}

impl BenchLiveRun {
    /// What it has cost so far, as far as its calls had a price.
    #[must_use]
    pub const fn cost(&self) -> BenchCost {
        BenchCost::new(self.cost_usd, self.unpriced_calls)
    }
}

/// What a run under way is doing right now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BenchDoing {
    /// `thinking`, `writing`, `waiting for the model`, `running
    /// run_command`, ...
    pub what: String,
    /// How long it has been at it
    pub for_ms: u64,
    /// The model call it is on, counting from 1; 0 while tools run
    pub call: u32,
    /// Times this model call started its reply over
    pub restarts: u32,
    /// What its subtask is doing, while one runs
    pub subtask: Option<Box<BenchDoing>>,
}
