//! One row of Gate's table, and rows added up or taken apart.

use super::observe::Counted;

/// One row of the table: the work of one model on one category at one
/// level. Every piece of work that ended counts for what it cost
/// (`turns`); those whose result is known count as tries too.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Row {
    /// Pieces of work that ended, whether or not their result is known.
    pub turns: u32,
    /// Those whose result is known.
    pub tries: u32,
    /// Those that succeeded.
    pub successes: u32,
    /// The list-price cost of the `turns`, in USD.
    pub cost_usd: f64,
    /// The list-price cost of the subtasks they dispatched, in USD.
    pub subtask_cost_usd: f64,
    /// Their wall time, in milliseconds.
    pub latency_ms: u64,
}

impl Row {
    /// Share of tries that succeeded; `None` before the first try.
    #[must_use]
    pub fn success_rate(&self) -> Option<f64> {
        (self.tries > 0).then(|| f64::from(self.successes) / f64::from(self.tries))
    }

    /// Mean cost per turn; `None` before the first turn.
    #[must_use]
    pub fn mean_cost_usd(&self) -> Option<f64> {
        (self.turns > 0).then(|| self.cost_usd / f64::from(self.turns))
    }

    /// Mean cost per turn with its subtasks'; `None` before the first turn.
    #[must_use]
    pub fn mean_total_cost_usd(&self) -> Option<f64> {
        (self.turns > 0).then(|| (self.cost_usd + self.subtask_cost_usd) / f64::from(self.turns))
    }

    /// Whether this row decides on its own: it has at least `min_tries`.
    #[must_use]
    pub const fn settled(&self, min_tries: u32) -> bool {
        self.tries >= min_tries
    }

    /// Whether the model can still have a success rate of at least
    /// `target` once it has `min_tries` tries: it does now, with that many
    /// tries or more, or would if every try it still lacks succeeded.
    #[must_use]
    pub fn can_still_reach(&self, target: f64, min_tries: u32) -> bool {
        let lacking = min_tries.saturating_sub(self.tries);
        let counted = self.tries.max(min_tries);
        f64::from(self.successes + lacking) >= target * f64::from(counted)
    }

    /// These outcomes without `other`'s, which must be among them.
    pub(super) fn without(&self, other: &Self) -> Self {
        Self {
            turns: self.turns.saturating_sub(other.turns),
            tries: self.tries.saturating_sub(other.tries),
            successes: self.successes.saturating_sub(other.successes),
            cost_usd: (self.cost_usd - other.cost_usd).max(0.0),
            subtask_cost_usd: (self.subtask_cost_usd - other.subtask_cost_usd).max(0.0),
            latency_ms: self.latency_ms.saturating_sub(other.latency_ms),
        }
    }

    /// These outcomes and `other`'s together.
    pub(super) fn with(&self, other: &Self) -> Self {
        Self {
            turns: self.turns + other.turns,
            tries: self.tries + other.tries,
            successes: self.successes + other.successes,
            cost_usd: self.cost_usd + other.cost_usd,
            subtask_cost_usd: self.subtask_cost_usd + other.subtask_cost_usd,
            latency_ms: self.latency_ms.saturating_add(other.latency_ms),
        }
    }

    /// Count `counted`, which does not take outcomes back, in the row.
    pub(super) fn add(&mut self, counted: &Counted) {
        self.turns += 1;
        self.cost_usd += counted.cost_usd;
        self.subtask_cost_usd += counted.subtask_cost_usd;
        self.latency_ms = self.latency_ms.saturating_add(counted.latency_ms);
        if let Some(success) = counted.success {
            self.tries += 1;
            self.successes += u32::from(success);
        }
    }

    /// Take `counted`, which [`Self::add`] counted, back out of the row.
    pub(super) fn remove(&mut self, counted: &Counted) {
        self.turns = self.turns.saturating_sub(1);
        self.cost_usd = (self.cost_usd - counted.cost_usd).max(0.0);
        self.subtask_cost_usd = (self.subtask_cost_usd - counted.subtask_cost_usd).max(0.0);
        self.latency_ms = self.latency_ms.saturating_sub(counted.latency_ms);
        if let Some(success) = counted.success {
            self.tries = self.tries.saturating_sub(1);
            self.successes = self.successes.saturating_sub(u32::from(success));
        }
    }
}
