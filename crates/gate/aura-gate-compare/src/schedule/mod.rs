//! Which runs start when, and the names they go by.

#[cfg(test)]
mod tests;

use aura_core_types::CompareArm;
use std::collections::VecDeque;

/// Runs that may work at the same time, across both arms.
pub const DEFAULT_CONCURRENCY: usize = 4;

/// What the finished runs have spent against the budget: the list-price
/// cost of their priced calls, and how many calls had no known price and
/// so are not counted in it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Spent {
    /// What the priced calls cost
    pub usd: f64,
    /// Calls with no known price, which `usd` (and so the budget) leaves
    /// out
    pub unpriced_calls: u32,
}

impl Spent {
    /// Whether every call had a price, so `usd` is all that was spent.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.unpriced_calls == 0
    }
}

/// Which runs start when: the arms alternate (`a-1`, `b-1`, `a-2`, ...)
/// so a budget that runs out leaves them balanced; at most `cap` work at
/// once; and once what the finished runs' priced calls cost reaches the
/// budget, no new run starts. Calls with no known price are counted but
/// never priced, so they cannot bring the budget closer.
#[derive(Debug, Clone)]
pub struct Schedule {
    pending: VecDeque<(CompareArm, u32)>,
    running: usize,
    cap: usize,
    budget_usd: Option<f64>,
    spent: Spent,
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
            spent: Spent::default(),
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

    /// A run ended, its priced calls having cost `cost_usd`, and
    /// `unpriced_calls` more having no known price.
    pub fn finished(&mut self, cost_usd: f64, unpriced_calls: u32) {
        self.running = self.running.saturating_sub(1);
        self.spent.usd += cost_usd.max(0.0);
        self.spent.unpriced_calls = self.spent.unpriced_calls.saturating_add(unpriced_calls);
    }

    /// Whether the budget is spent, by the priced calls alone: an unknown
    /// price is not guessed at.
    fn over_budget(&self) -> bool {
        self.budget_usd
            .is_some_and(|budget| self.spent.usd >= budget)
    }

    /// What the finished runs spent so far, and the calls that leaves out.
    #[must_use]
    pub const fn spent(&self) -> Spent {
        self.spent
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
