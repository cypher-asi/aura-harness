//! What Gate has learned: per level, category and model, how many
//! counted outcomes there were, how many succeeded, and what they cost.
//!
//! [`GateStats`] is built by feeding it record entries
//! ([`GateStats::observe`]): the log at startup, then every entry the
//! kernels commit. Each prompt counts once, with its effective outcome
//! (the user's latest rating, else the latest outcome with a known
//! result, as [`crate::outcome::effective`] says). An outcome that
//! replaces an earlier one for the same prompt takes its place in the
//! table instead of adding to it.
//!
//! Entries may arrive in any order and more than once (a startup scan
//! can overlap the live feed): which outcome counts depends only on the
//! record sequence numbers, and seeing an entry again changes nothing.
//! A prompt's outcomes are in the same record as the prompt, so their
//! sequence numbers compare. A prompt is known by its agent and its
//! transaction hash: the hash covers only the payload, so two agents
//! given the same prompt (the repeats of a `/compare`) record the same
//! hash for different work.

use std::collections::HashMap;

use aura_core_types::{AgentId, Hash, OutcomePayload, PromptCategory, RecordEntry, RouteLevel};

/// One row of the table: the counted outcomes of one model on one
/// category at one level.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Row {
    /// Counted outcomes.
    pub tries: u32,
    /// Those that succeeded.
    pub successes: u32,
    /// Their list-price cost, in USD.
    pub cost_usd: f64,
    /// Their wall time, in milliseconds.
    pub latency_ms: u64,
}

impl Row {
    /// Share of tries that succeeded; `None` before the first try.
    #[must_use]
    pub fn success_rate(&self) -> Option<f64> {
        (self.tries > 0).then(|| f64::from(self.successes) / f64::from(self.tries))
    }

    /// Mean cost per try; `None` before the first try.
    #[must_use]
    pub fn mean_cost_usd(&self) -> Option<f64> {
        (self.tries > 0).then(|| self.cost_usd / f64::from(self.tries))
    }

    fn add(&mut self, counted: &Counted) {
        self.tries += 1;
        self.successes += u32::from(counted.success);
        self.cost_usd += counted.cost_usd;
        self.latency_ms = self.latency_ms.saturating_add(counted.latency_ms);
    }

    fn remove(&mut self, counted: &Counted) {
        self.tries = self.tries.saturating_sub(1);
        self.successes = self.successes.saturating_sub(u32::from(counted.success));
        self.cost_usd = (self.cost_usd - counted.cost_usd).max(0.0);
        self.latency_ms = self.latency_ms.saturating_sub(counted.latency_ms);
    }
}

/// Which row an outcome counts in.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RowKey {
    /// Turn or subtask.
    pub level: RouteLevel,
    /// What the prompt asked for.
    pub category: PromptCategory,
    /// Model that did the work.
    pub model: String,
}

/// The outcome a prompt is counted with.
#[derive(Debug, Clone, PartialEq)]
struct Counted {
    /// Record sequence number of the outcome.
    seq: u64,
    /// Whether it is the user's rating, which wins over later automatic
    /// outcomes.
    rating: bool,
    category: PromptCategory,
    model: String,
    success: bool,
    cost_usd: f64,
    latency_ms: u64,
}

impl Counted {
    fn from_outcome(seq: u64, outcome: OutcomePayload) -> Option<Self> {
        Some(Self {
            seq,
            rating: outcome.signal.is_rating(),
            category: outcome.category,
            success: outcome.success?,
            model: outcome.model,
            cost_usd: outcome.cost_usd,
            latency_ms: outcome.latency_ms,
        })
    }

    /// Whether `self` counts instead of `other` for the same prompt.
    fn supersedes(&self, other: &Self) -> bool {
        match (self.rating, other.rating) {
            (true, false) => true,
            (false, true) => false,
            _ => self.seq > other.seq,
        }
    }

    fn key(&self, level: RouteLevel) -> RowKey {
        RowKey {
            level,
            category: self.category,
            model: self.model.clone(),
        }
    }
}

/// What is known about one prompt.
#[derive(Debug, Clone, Default, PartialEq)]
struct PromptState {
    /// From the prompt's route, once the prompt has been seen.
    level: Option<RouteLevel>,
    /// The outcome it counts with, once one with a known result has
    /// been seen.
    counted: Option<Counted>,
}

/// Gate's table, and the per-prompt outcomes it is built from.
#[derive(Debug, Clone, Default)]
pub struct GateStats {
    rows: HashMap<RowKey, Row>,
    prompts: HashMap<(AgentId, Hash), PromptState>,
}

impl GateStats {
    /// The table built from `entries`.
    #[must_use]
    pub fn from_entries<'a>(entries: impl IntoIterator<Item = &'a RecordEntry>) -> Self {
        let mut stats = Self::default();
        for entry in entries {
            stats.observe(entry);
        }
        stats
    }

    /// Take `entry` into account: a prompt says the level of its route,
    /// and an outcome with a known result may become what its prompt
    /// counts with. Other entries are ignored.
    pub fn observe(&mut self, entry: &RecordEntry) {
        let agent = entry.tx.agent_id;
        if let Some(prompt) = entry.tx.prompt_payload() {
            self.observe_prompt((agent, entry.tx.hash), prompt.route.level);
        } else if let Some(outcome) = entry.tx.outcome_payload() {
            self.observe_outcome(agent, entry.seq, outcome);
        }
    }

    fn observe_prompt(&mut self, prompt: (AgentId, Hash), level: RouteLevel) {
        let state = self.prompts.entry(prompt).or_default();
        if state.level.is_some() {
            return;
        }
        state.level = Some(level);
        if let Some(counted) = &state.counted {
            self.rows
                .entry(counted.key(level))
                .or_default()
                .add(counted);
        }
    }

    fn observe_outcome(&mut self, agent: AgentId, seq: u64, outcome: OutcomePayload) {
        let prompt_tx = outcome.prompt_tx;
        let Some(new) = Counted::from_outcome(seq, outcome) else {
            return;
        };
        let state = self.prompts.entry((agent, prompt_tx)).or_default();
        if state
            .counted
            .as_ref()
            .is_some_and(|old| !new.supersedes(old))
        {
            return;
        }
        if let Some(level) = state.level {
            if let Some(old) = &state.counted {
                let key = old.key(level);
                if let Some(row) = self.rows.get_mut(&key) {
                    row.remove(old);
                    if row.tries == 0 {
                        self.rows.remove(&key);
                    }
                }
            }
            self.rows.entry(new.key(level)).or_default().add(&new);
        }
        state.counted = Some(new);
    }

    /// The row of `model` on `category` at `level`, if it has tries.
    #[must_use]
    pub fn row(&self, level: RouteLevel, category: PromptCategory, model: &str) -> Option<&Row> {
        self.rows.get(&RowKey {
            level,
            category,
            model: model.to_string(),
        })
    }

    /// Mean cost per try of `model` at `level`, over every category;
    /// `None` before its first try there.
    #[must_use]
    pub fn mean_cost_usd(&self, level: RouteLevel, model: &str) -> Option<f64> {
        let (tries, cost) = self
            .rows
            .iter()
            .filter(|(key, _)| key.level == level && key.model == model)
            .fold((0_u32, 0.0), |(tries, cost), (_, row)| {
                (tries + row.tries, cost + row.cost_usd)
            });
        (tries > 0).then(|| cost / f64::from(tries))
    }

    /// Every row with tries, by level, category and model.
    #[must_use]
    pub fn rows(&self) -> Vec<(&RowKey, &Row)> {
        let mut rows: Vec<(&RowKey, &Row)> = self.rows.iter().collect();
        rows.sort_by(|a, b| a.0.cmp(b.0));
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::{AgentId, OutcomeSignal, PromptPayload, Route, RouteReason, Transaction};

    const SONNET: &str = "claude/claude-sonnet-4-6";
    const HAIKU: &str = "claude/claude-haiku-4-5";

    /// Builds one agent's record.
    struct Log {
        agent: AgentId,
        entries: Vec<RecordEntry>,
    }

    impl Log {
        fn new() -> Self {
            Self {
                agent: AgentId::generate(),
                entries: Vec::new(),
            }
        }

        fn push(&mut self, tx: Transaction) -> Hash {
            let hash = tx.hash;
            let seq = u64::try_from(self.entries.len()).unwrap() + 1;
            self.entries.push(RecordEntry::builder(seq, tx).build());
            hash
        }

        fn prompt(&mut self, text: &str, level: RouteLevel) -> Hash {
            let mut payload = PromptPayload::new(text);
            payload.category = PromptCategory::Debugging;
            payload.route = Route {
                level,
                ..Route::turn(SONNET, RouteReason::Default)
            };
            self.push(Transaction::user_prompt_with(self.agent, &payload))
        }

        fn outcome(&mut self, prompt_tx: Hash, model: &str, signal: OutcomeSignal) {
            let outcome = OutcomePayload {
                prompt_tx,
                category: PromptCategory::Debugging,
                model: model.to_string(),
                success: signal.success(),
                signal,
                cost_usd: 0.25,
                latency_ms: 1_000,
            };
            self.push(Transaction::outcome(self.agent, &outcome));
        }
    }

    fn debugging(stats: &GateStats, level: RouteLevel, model: &str) -> Option<(u32, u32)> {
        stats
            .row(level, PromptCategory::Debugging, model)
            .map(|row| (row.tries, row.successes))
    }

    #[test]
    fn each_prompt_counts_once_with_its_latest_known_outcome() {
        let mut log = Log::new();
        let first = log.prompt("fix the parser", RouteLevel::Turn);
        log.outcome(first, SONNET, OutcomeSignal::ChecksPassed);
        log.outcome(first, SONNET, OutcomeSignal::Rewind);
        let second = log.prompt("fix the lexer", RouteLevel::Turn);
        log.outcome(second, SONNET, OutcomeSignal::None);
        let third = log.prompt("fix the build", RouteLevel::Turn);
        log.outcome(third, SONNET, OutcomeSignal::ChecksPassed);

        let stats = GateStats::from_entries(&log.entries);
        assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((2, 1)));
        let row = stats
            .row(RouteLevel::Turn, PromptCategory::Debugging, SONNET)
            .unwrap();
        assert_eq!(row.success_rate(), Some(0.5));
        assert_eq!(row.mean_cost_usd(), Some(0.25));
        assert_eq!(row.latency_ms, 2_000);
    }

    #[test]
    fn a_rating_wins_over_later_automatic_outcomes() {
        let mut log = Log::new();
        let prompt = log.prompt("fix it", RouteLevel::Turn);
        log.outcome(prompt, SONNET, OutcomeSignal::ChecksFailed);
        log.outcome(prompt, SONNET, OutcomeSignal::Good);
        log.outcome(prompt, SONNET, OutcomeSignal::RetryStronger);
        let stats = GateStats::from_entries(&log.entries);
        assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((1, 1)));
    }

    #[test]
    fn superseding_outcomes_replace_rather_than_add() {
        let mut log = Log::new();
        let prompt = log.prompt("fix it", RouteLevel::Turn);
        log.outcome(prompt, SONNET, OutcomeSignal::ChecksPassed);
        let mut stats = GateStats::default();
        for entry in &log.entries {
            stats.observe(entry);
        }
        assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((1, 1)));

        log.outcome(prompt, SONNET, OutcomeSignal::Bad);
        stats.observe(log.entries.last().unwrap());
        assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((1, 0)));

        log.outcome(prompt, SONNET, OutcomeSignal::Good);
        stats.observe(log.entries.last().unwrap());
        stats.observe(log.entries.last().unwrap());
        assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((1, 1)));
        assert_eq!(stats.rows().len(), 1);
    }

    #[test]
    fn a_replacing_outcome_on_another_model_moves_the_count() {
        let mut log = Log::new();
        let prompt = log.prompt("fix it", RouteLevel::Turn);
        log.outcome(prompt, HAIKU, OutcomeSignal::ChecksPassed);
        log.outcome(prompt, SONNET, OutcomeSignal::ChecksFailed);
        let stats = GateStats::from_entries(&log.entries);
        assert_eq!(debugging(&stats, RouteLevel::Turn, HAIKU), None);
        assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((1, 0)));
    }

    #[test]
    fn order_and_repeats_do_not_change_the_table() {
        let mut log = Log::new();
        for text in ["fix a", "fix b", "fix c"] {
            let prompt = log.prompt(text, RouteLevel::Turn);
            log.outcome(prompt, SONNET, OutcomeSignal::ChecksPassed);
            log.outcome(prompt, SONNET, OutcomeSignal::Rewind);
            log.outcome(prompt, SONNET, OutcomeSignal::Good);
        }
        let in_order = GateStats::from_entries(&log.entries);
        let reversed = GateStats::from_entries(log.entries.iter().rev());
        let twice = GateStats::from_entries(log.entries.iter().chain(&log.entries));
        for stats in [&reversed, &twice] {
            assert_eq!(stats.rows(), in_order.rows());
        }
        assert_eq!(debugging(&in_order, RouteLevel::Turn, SONNET), Some((3, 3)));
    }

    #[test]
    fn the_same_prompt_in_two_agents_counts_twice() {
        let mut first = Log::new();
        let mut second = Log::new();
        let a = first.prompt("fix it", RouteLevel::Turn);
        let b = second.prompt("fix it", RouteLevel::Turn);
        assert_eq!(a, b, "the hash covers only the payload");
        first.outcome(a, SONNET, OutcomeSignal::ChecksPassed);
        second.outcome(b, SONNET, OutcomeSignal::ChecksFailed);
        let stats = GateStats::from_entries(first.entries.iter().chain(&second.entries));
        assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((2, 1)));
    }

    #[test]
    fn levels_learn_separately() {
        let mut log = Log::new();
        let turn = log.prompt("fix it", RouteLevel::Turn);
        log.outcome(turn, SONNET, OutcomeSignal::ChecksPassed);
        let subtask = log.prompt("fix that part", RouteLevel::Subtask);
        log.outcome(subtask, SONNET, OutcomeSignal::ChecksFailed);
        let stats = GateStats::from_entries(&log.entries);
        assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((1, 1)));
        assert_eq!(debugging(&stats, RouteLevel::Subtask, SONNET), Some((1, 0)));
        assert_eq!(stats.mean_cost_usd(RouteLevel::Turn, SONNET), Some(0.25));
        assert_eq!(stats.mean_cost_usd(RouteLevel::Turn, HAIKU), None);
    }
}
