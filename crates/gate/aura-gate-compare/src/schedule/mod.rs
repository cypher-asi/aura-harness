//! Which runs start when, and the names they go by.

#[cfg(test)]
mod tests;

use aura_core_types::CompareArm;
use std::collections::VecDeque;

/// Runs that may work at the same time, across both arms.
pub const DEFAULT_CONCURRENCY: usize = 4;

/// Which runs start when: the arms alternate (`a-1`, `b-1`, `a-2`, ...)
/// so a budget that runs out leaves them balanced; at most `cap` work at
/// once; and once what the finished runs cost reaches the budget, no new
/// run starts.
#[derive(Debug, Clone)]
pub struct Schedule {
    pending: VecDeque<(CompareArm, u32)>,
    running: usize,
    cap: usize,
    budget_usd: Option<f64>,
    spent_usd: f64,
}

impl Schedule {
    /// `repeats` runs per arm, `cap` at a time, stopping at `budget_usd`.
    #[must_use]
    pub fn new(repeats: u32, cap: usize, budget_usd: Option<f64>) -> Self {
        let pending = (1..=repeats)
            .flat_map(|n| CompareArm::BOTH.map(|arm| (arm, n)))
            .collect();
        Self {
            pending,
            running: 0,
            cap: cap.max(1),
            budget_usd,
            spent_usd: 0.0,
        }
    }

    /// The next run to start now, if one may: there is room under the
    /// cap and money left in the budget. Counts it as running.
    pub fn start_next(&mut self) -> Option<(CompareArm, u32)> {
        if self.running >= self.cap || self.over_budget() {
            return None;
        }
        let next = self.pending.pop_front()?;
        self.running += 1;
        Some(next)
    }

    /// A run ended, having cost `cost_usd`.
    pub fn finished(&mut self, cost_usd: f64) {
        self.running = self.running.saturating_sub(1);
        self.spent_usd += cost_usd.max(0.0);
    }

    /// Whether the budget is spent.
    fn over_budget(&self) -> bool {
        self.budget_usd
            .is_some_and(|budget| self.spent_usd >= budget)
    }

    /// What the finished runs cost so far.
    #[must_use]
    pub const fn spent_usd(&self) -> f64 {
        self.spent_usd
    }

    /// Drop the runs that have not started (the comparison is stopped,
    /// or the budget ran out) and return them.
    pub fn drop_pending(&mut self) -> Vec<(CompareArm, u32)> {
        self.pending.drain(..).collect()
    }
}

/// The name of run `n` of `arm`: `a-1`, `b-2`.
#[must_use]
pub fn run_name(arm: CompareArm, n: u32) -> String {
    format!("{arm}-{n}")
}
