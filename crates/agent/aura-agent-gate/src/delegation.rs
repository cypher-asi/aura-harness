//! How much of a lead's work went to subtasks: per turn and overall, the
//! cost and model calls of the lead itself and of the subtasks it
//! dispatched, how many subtasks each turn dispatched, and how often a
//! subtask was retried on a stronger model.
//!
//! The app reads the lead's record (its turn prompts and model calls)
//! and those of its subagents (each one's prompt and model calls), and
//! [`summarize`] puts each subtask in the lead's turn its prompt was
//! recorded in.

/// A model call the lead made.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Call {
    /// When it was recorded, in Unix milliseconds.
    pub ts_ms: u64,
    /// Its list-price cost in USD.
    pub cost_usd: f64,
}

/// A subtask the lead dispatched, directly or through its subtasks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Subtask {
    /// When its prompt was recorded, in Unix milliseconds.
    pub ts_ms: u64,
    /// Whether it retried a failed subtask on a stronger model.
    pub escalated: bool,
    /// Its model calls.
    pub calls: u32,
    /// Their list-price cost in USD.
    pub cost_usd: f64,
}

/// The lead's and its subtasks' part of some work.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Share {
    /// Model calls of the lead.
    pub lead_calls: u32,
    /// What they cost.
    pub lead_cost_usd: f64,
    /// Model calls of the subtasks, escalated retries included.
    pub subtask_calls: u32,
    /// What they cost.
    pub subtask_cost_usd: f64,
    /// Subtasks dispatched, not counting escalated retries.
    pub subtasks: u32,
    /// Escalated retries.
    pub escalations: u32,
}

impl Share {
    /// Share of the cost spent in subtasks; `None` when nothing cost
    /// anything.
    #[must_use]
    pub fn cost_share(&self) -> Option<f64> {
        let total = self.lead_cost_usd + self.subtask_cost_usd;
        (total > 0.0).then(|| self.subtask_cost_usd / total)
    }

    /// Share of the model calls made in subtasks; `None` without calls.
    #[must_use]
    pub fn call_share(&self) -> Option<f64> {
        let total = self.lead_calls + self.subtask_calls;
        (total > 0).then(|| f64::from(self.subtask_calls) / f64::from(total))
    }

    /// Share of subtasks that were retried on a stronger model; `None`
    /// without subtasks.
    #[must_use]
    pub fn escalation_rate(&self) -> Option<f64> {
        (self.subtasks > 0).then(|| f64::from(self.escalations) / f64::from(self.subtasks))
    }

    fn add(&mut self, other: &Self) {
        self.lead_calls += other.lead_calls;
        self.lead_cost_usd += other.lead_cost_usd;
        self.subtask_calls += other.subtask_calls;
        self.subtask_cost_usd += other.subtask_cost_usd;
        self.subtasks += other.subtasks;
        self.escalations += other.escalations;
    }
}

/// One turn of the lead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TurnShare {
    /// When its prompt was recorded, in Unix milliseconds.
    pub ts_ms: u64,
    pub share: Share,
}

/// A lead's delegation, per turn and overall.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Delegation {
    /// Oldest first.
    pub turns: Vec<TurnShare>,
    pub overall: Share,
}

impl Delegation {
    /// Subtasks dispatched per turn, escalated retries left out; `None`
    /// before the first turn.
    #[must_use]
    pub fn subtasks_per_turn(&self) -> Option<f64> {
        let turns = u32::try_from(self.turns.len()).unwrap_or(u32::MAX);
        (turns > 0).then(|| f64::from(self.overall.subtasks) / f64::from(turns))
    }
}

/// The delegation of a lead whose turns started at `turn_starts`, that
/// made `calls`, and whose work dispatched `subtasks`. Each call and
/// subtask belongs to the turn that started last before it (the first
/// turn for anything earlier).
#[must_use]
pub fn summarize(turn_starts: &[u64], calls: &[Call], subtasks: &[Subtask]) -> Delegation {
    let mut starts = turn_starts.to_vec();
    starts.sort_unstable();
    let mut turns: Vec<TurnShare> = starts
        .iter()
        .map(|&ts_ms| TurnShare {
            ts_ms,
            share: Share::default(),
        })
        .collect();
    if turns.is_empty() {
        return Delegation::default();
    }
    let turn_of = |ts_ms: u64| starts.partition_point(|&start| start <= ts_ms).max(1) - 1;
    for call in calls {
        let share = &mut turns[turn_of(call.ts_ms)].share;
        share.lead_calls += 1;
        share.lead_cost_usd += call.cost_usd;
    }
    for subtask in subtasks {
        let share = &mut turns[turn_of(subtask.ts_ms)].share;
        share.subtask_calls += subtask.calls;
        share.subtask_cost_usd += subtask.cost_usd;
        if subtask.escalated {
            share.escalations += 1;
        } else {
            share.subtasks += 1;
        }
    }
    let mut overall = Share::default();
    for turn in &turns {
        overall.add(&turn.share);
    }
    Delegation { turns, overall }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(ts_ms: u64, cost_usd: f64) -> Call {
        Call { ts_ms, cost_usd }
    }

    fn subtask(ts_ms: u64, escalated: bool, calls: u32, cost_usd: f64) -> Subtask {
        Subtask {
            ts_ms,
            escalated,
            calls,
            cost_usd,
        }
    }

    #[test]
    fn calls_and_subtasks_belong_to_the_turn_they_happened_in() {
        let delegation = summarize(
            &[100, 200],
            &[call(110, 0.10), call(150, 0.10), call(210, 0.20)],
            &[
                subtask(120, false, 3, 0.30),
                subtask(130, false, 2, 0.10),
                subtask(140, true, 4, 0.40),
                subtask(220, false, 1, 0.05),
            ],
        );
        assert_eq!(delegation.turns.len(), 2);
        let first = delegation.turns[0].share;
        assert_eq!(first.lead_calls, 2);
        assert_eq!(first.subtasks, 2);
        assert_eq!(first.escalations, 1);
        assert_eq!(first.subtask_calls, 9);
        assert!((first.cost_share().unwrap() - 0.8 / 1.0).abs() < 1e-9);
        assert_eq!(first.escalation_rate(), Some(0.5));

        let overall = delegation.overall;
        assert_eq!(overall.lead_calls, 3);
        assert_eq!(overall.subtask_calls, 10);
        assert_eq!(overall.subtasks, 3);
        assert!((overall.call_share().unwrap() - 10.0 / 13.0).abs() < 1e-9);
        assert!((overall.cost_share().unwrap() - 0.85 / 1.25).abs() < 1e-9);
        assert_eq!(delegation.subtasks_per_turn(), Some(1.5));
    }

    #[test]
    fn work_before_the_first_turn_counts_in_it_and_no_turns_say_nothing() {
        let delegation = summarize(&[100], &[call(90, 0.01)], &[subtask(50, false, 1, 0.02)]);
        assert_eq!(delegation.turns[0].share.lead_calls, 1);
        assert_eq!(delegation.turns[0].share.subtasks, 1);

        let none = summarize(&[], &[call(1, 1.0)], &[]);
        assert!(none.turns.is_empty());
        assert_eq!(none.subtasks_per_turn(), None);
        assert_eq!(none.overall.cost_share(), None);
        assert_eq!(none.overall.call_share(), None);
        assert_eq!(none.overall.escalation_rate(), None);
    }
}
