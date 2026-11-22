//! How each arm did on each prompt of a replayed session, and on each
//! example task.

use super::mean;
use aura_core_types::{CompareArm, ComparePayload, OutcomeSignal};

/// How the runs of one arm did on one prompt.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnCell {
    /// Runs that got to the prompt.
    pub runs: usize,
    /// Mean cost of their turns.
    pub mean_cost_usd: Option<f64>,
    /// Turns whose last test or build command passed.
    pub passed: usize,
    /// Turns whose last test or build command failed.
    pub failed: usize,
    /// Turns that ended in an error.
    pub errors: usize,
}

/// One cell per prompt of `payload`, for the runs of `arm`.
#[must_use]
pub fn turn_cells(payload: &ComparePayload, arm: CompareArm) -> Vec<TurnCell> {
    let runs: Vec<_> = payload.arm_runs(arm).collect();
    (0..payload.prompts.len())
        .map(|i| {
            let turns: Vec<_> = runs.iter().filter_map(|run| run.turns.get(i)).collect();
            let count = |signal: OutcomeSignal| turns.iter().filter(|t| t.signal == signal).count();
            TurnCell {
                runs: turns.len(),
                mean_cost_usd: mean(turns.iter().map(|t| t.cost_usd)),
                passed: count(OutcomeSignal::ChecksPassed),
                failed: count(OutcomeSignal::ChecksFailed),
                errors: count(OutcomeSignal::Error),
            }
        })
        .collect()
}

/// How the runs of one arm did on one example task.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskCell {
    /// Runs that worked on it.
    pub runs: usize,
    /// Runs whose check passed.
    pub passed: usize,
    /// Runs whose check ran.
    pub checked: usize,
    /// Mean cost of its runs.
    pub mean_cost_usd: Option<f64>,
}

/// One cell per example task of `payload`, for the runs of `arm`; empty
/// unless the work is example tasks.
#[must_use]
pub fn task_cells(payload: &ComparePayload, arm: CompareArm) -> Vec<TaskCell> {
    let arm_runs: Vec<_> = payload.arm_runs(arm).collect();
    payload
        .source
        .examples()
        .iter()
        .map(|task| {
            let runs: Vec<_> = arm_runs
                .iter()
                .filter(|run| run.task.as_deref() == Some(task.id.as_str()))
                .collect();
            TaskCell {
                runs: runs.len(),
                passed: runs.iter().filter(|r| r.check_passed == Some(true)).count(),
                checked: runs.iter().filter(|r| r.check_passed.is_some()).count(),
                mean_cost_usd: mean(runs.iter().map(|r| r.cost_usd)),
            }
        })
        .collect()
}
