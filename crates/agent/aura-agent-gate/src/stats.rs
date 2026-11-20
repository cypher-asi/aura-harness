//! What Gate has learned: per level, category and model, how many
//! counted outcomes there were, how many succeeded, and what they cost.
//!
//! [`GateStats`] is built by feeding it record entries
//! ([`GateStats::observe`]): the log at startup, then every entry the
//! kernels commit. Each prompt counts once, with its effective outcome
//! (the user's latest rating, else the latest outcome with a known
//! result, as [`crate::outcome::effective`] says). An outcome that
//! replaces an earlier one for the same prompt takes its place in the
//! table instead of adding to it, and one that takes the earlier ones
//! back (the judge found the failure was not the model's doing, see
//! [`aura_core_types::OutcomeSignal::voids`]) leaves the prompt out of
//! the table.
//!
//! Entries may arrive in any order and more than once (a startup scan
//! can overlap the live feed): which outcome counts depends only on the
//! record sequence numbers, and seeing an entry again changes nothing.
//! A prompt's outcomes are in the same record as the prompt, so their
//! sequence numbers compare. Work that went on to a stronger model
//! after its first attempt failed (route reason `escalate`) is not
//! counted: it only ever sees work that already failed once, and the
//! first attempt's outcome already says how the route did. A prompt is
//! known by its agent and its transaction hash: the hash covers only the payload, so two agents
//! given the same prompt (the repeats of a `/compare`) record the same
//! hash for different work.

use std::collections::HashMap;

use crate::replay::Logged;
use aura_core_types::{
    AgentId, Difficulty, Hash, OutcomePayload, PromptCategory, RecordEntry, RouteLevel, RouteReason,
};

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

    /// Mean cost per try; `None` before the first try.
    #[must_use]
    pub fn mean_cost_usd(&self) -> Option<f64> {
        (self.tries > 0).then(|| self.cost_usd / f64::from(self.tries))
    }

    /// Mean cost per try with its subtasks'; `None` before the first try.
    #[must_use]
    pub fn mean_total_cost_usd(&self) -> Option<f64> {
        (self.tries > 0).then(|| (self.cost_usd + self.subtask_cost_usd) / f64::from(self.tries))
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
    #[must_use]
    pub fn without(&self, other: &Self) -> Self {
        Self {
            tries: self.tries.saturating_sub(other.tries),
            successes: self.successes.saturating_sub(other.successes),
            cost_usd: (self.cost_usd - other.cost_usd).max(0.0),
            subtask_cost_usd: (self.subtask_cost_usd - other.subtask_cost_usd).max(0.0),
            latency_ms: self.latency_ms.saturating_sub(other.latency_ms),
        }
    }

    fn add(&mut self, counted: &Counted) {
        let Some(success) = counted.success else {
            return;
        };
        self.tries += 1;
        self.successes += u32::from(success);
        self.cost_usd += counted.cost_usd;
        self.subtask_cost_usd += counted.subtask_cost_usd;
        self.latency_ms = self.latency_ms.saturating_add(counted.latency_ms);
    }

    fn remove(&mut self, counted: &Counted) {
        let Some(success) = counted.success else {
            return;
        };
        self.tries = self.tries.saturating_sub(1);
        self.successes = self.successes.saturating_sub(u32::from(success));
        self.cost_usd = (self.cost_usd - counted.cost_usd).max(0.0);
        self.subtask_cost_usd = (self.subtask_cost_usd - counted.subtask_cost_usd).max(0.0);
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

/// A finer row of the table: one model on one category at one level, for
/// one kind of subagent (`None` for a turn) done one way (led or not).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DetailKey {
    pub level: RouteLevel,
    pub category: PromptCategory,
    pub model: String,
    /// The permanent agent a subtask went to
    pub kind: Option<String>,
    /// Whether the turn was led; `None` when its prompt does not say
    pub led: Option<bool>,
}

/// What a prompt says about how its work was done.
#[derive(Debug, Clone, PartialEq)]
struct Shape {
    level: RouteLevel,
    kind: Option<String>,
    led: Option<bool>,
    difficulty: Option<Difficulty>,
    /// How likely the router was to pick its model
    propensity: f64,
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
    /// `None` when the outcome takes back what was said before: the
    /// prompt then counts in no row.
    success: Option<bool>,
    cost_usd: f64,
    subtask_cost_usd: f64,
    latency_ms: u64,
}

impl Counted {
    fn from_outcome(seq: u64, outcome: OutcomePayload) -> Option<Self> {
        if outcome.success.is_none() && !outcome.signal.voids() {
            return None;
        }
        Some(Self {
            seq,
            rating: outcome.signal.is_rating(),
            category: outcome.category,
            success: outcome.success,
            model: outcome.model,
            cost_usd: outcome.cost_usd,
            subtask_cost_usd: outcome.subtask_cost_usd,
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

    fn detail_key(&self, shape: &Shape) -> DetailKey {
        DetailKey {
            level: shape.level,
            category: self.category,
            model: self.model.clone(),
            kind: shape.kind.clone(),
            led: shape.led,
        }
    }
}

/// What is known about one prompt.
#[derive(Debug, Clone, Default, PartialEq)]
struct PromptState {
    /// From the prompt, once it has been seen.
    shape: Option<Shape>,
    /// The prompt is an escalated retry, which is never counted.
    ignored: bool,
    /// The outcome it counts with, once one with a known result has
    /// been seen.
    counted: Option<Counted>,
}

/// Gate's table, and the per-prompt outcomes it is built from.
#[derive(Debug, Clone, Default)]
pub struct GateStats {
    rows: HashMap<RowKey, Row>,
    /// The same outcomes, split by kind of subagent and way of working
    details: HashMap<DetailKey, Row>,
    /// The outcomes of prompts whose difficulty is known, by level, model
    /// and difficulty, over every category.
    difficulties: HashMap<(RouteLevel, String, Difficulty), Row>,
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
            if prompt.route.reason == RouteReason::Escalate {
                self.prompts
                    .entry((agent, entry.tx.hash))
                    .or_default()
                    .ignored = true;
                return;
            }
            let shape = Shape {
                level: prompt.route.level,
                kind: prompt
                    .kind
                    .filter(|_| prompt.route.level == RouteLevel::Subtask),
                led: prompt
                    .led
                    .filter(|_| prompt.route.level == RouteLevel::Turn),
                difficulty: prompt.difficulty,
                propensity: prompt.route.propensity,
            };
            self.observe_prompt((agent, entry.tx.hash), shape);
        } else if let Some(outcome) = entry.tx.outcome_payload() {
            self.observe_outcome(agent, entry.seq, outcome);
        }
    }

    fn observe_prompt(&mut self, prompt: (AgentId, Hash), shape: Shape) {
        let state = self.prompts.entry(prompt).or_default();
        if state.shape.is_some() {
            return;
        }
        if let Some(counted) = state.counted.clone().filter(|c| c.success.is_some()) {
            self.add(&counted, &shape);
        }
        if let Some(state) = self.prompts.get_mut(&prompt) {
            state.shape = Some(shape);
        }
    }

    fn add(&mut self, counted: &Counted, shape: &Shape) {
        self.rows
            .entry(counted.key(shape.level))
            .or_default()
            .add(counted);
        self.details
            .entry(counted.detail_key(shape))
            .or_default()
            .add(counted);
        if let Some(difficulty) = shape.difficulty {
            self.difficulties
                .entry((shape.level, counted.model.clone(), difficulty))
                .or_default()
                .add(counted);
        }
    }

    fn remove(&mut self, counted: &Counted, shape: &Shape) {
        let key = counted.key(shape.level);
        if let Some(row) = self.rows.get_mut(&key) {
            row.remove(counted);
            if row.tries == 0 {
                self.rows.remove(&key);
            }
        }
        let key = counted.detail_key(shape);
        if let Some(row) = self.details.get_mut(&key) {
            row.remove(counted);
            if row.tries == 0 {
                self.details.remove(&key);
            }
        }
        if let Some(difficulty) = shape.difficulty {
            let key = (shape.level, counted.model.clone(), difficulty);
            if let Some(row) = self.difficulties.get_mut(&key) {
                row.remove(counted);
                if row.tries == 0 {
                    self.difficulties.remove(&key);
                }
            }
        }
    }

    fn observe_outcome(&mut self, agent: AgentId, seq: u64, outcome: OutcomePayload) {
        let prompt_tx = outcome.prompt_tx;
        let Some(new) = Counted::from_outcome(seq, outcome) else {
            return;
        };
        let state = self.prompts.entry((agent, prompt_tx)).or_default();
        if state.ignored {
            return;
        }
        if state
            .counted
            .as_ref()
            .is_some_and(|old| !new.supersedes(old))
        {
            return;
        }
        let (shape, old) = (state.shape.clone(), state.counted.replace(new.clone()));
        if let Some(shape) = shape {
            if let Some(old) = old.filter(|old| old.success.is_some()) {
                self.remove(&old, &shape);
            }
            if new.success.is_some() {
                self.add(&new, &shape);
            }
        }
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

    /// The row of `model` on `category` for subtasks that went to the
    /// permanent agent `kind`, if it has tries.
    #[must_use]
    pub fn kind_row(&self, category: PromptCategory, kind: &str, model: &str) -> Option<&Row> {
        self.details.get(&DetailKey {
            level: RouteLevel::Subtask,
            category,
            model: model.to_string(),
            kind: Some(kind.to_string()),
            led: None,
        })
    }

    /// The row of `model` on turns asking for `category` that were led
    /// (`led`) or done by the model itself, if it has tries. Turns whose
    /// prompt does not say are in neither.
    #[must_use]
    pub fn way_row(&self, category: PromptCategory, model: &str, led: bool) -> Option<&Row> {
        self.details.get(&DetailKey {
            level: RouteLevel::Turn,
            category,
            model: model.to_string(),
            kind: None,
            led: Some(led),
        })
    }

    /// The row of `model` on turns asking for `category` that it did
    /// itself: every turn but those it led, whose outcome says as much
    /// about the subtasks as about the lead. `None` when there are none.
    #[must_use]
    pub fn unled_row(&self, category: PromptCategory, model: &str) -> Option<Row> {
        let all = self.row(RouteLevel::Turn, category, model)?;
        let row = self
            .way_row(category, model, true)
            .map_or(*all, |led| all.without(led));
        (row.tries > 0).then_some(row)
    }

    /// Every finer row with tries, by level, category, model, kind and way.
    #[must_use]
    pub fn details(&self) -> Vec<(&DetailKey, &Row)> {
        let mut rows: Vec<(&DetailKey, &Row)> = self.details.iter().collect();
        rows.sort_by(|a, b| a.0.cmp(b.0));
        rows
    }

    /// The outcomes of `model` at `level` on work the classifier judged of
    /// `difficulty`, over every category, if it has tries.
    #[must_use]
    pub fn difficulty_row(
        &self,
        level: RouteLevel,
        model: &str,
        difficulty: Difficulty,
    ) -> Option<&Row> {
        self.difficulties
            .get(&(level, model.to_string(), difficulty))
    }

    /// Every counted outcome of `model` at `level`, over every category.
    #[must_use]
    pub fn level_row(&self, level: RouteLevel, model: &str) -> Row {
        self.rows
            .iter()
            .filter(|(key, _)| key.level == level && key.model == model)
            .fold(Row::default(), |sum, (_, row)| Row {
                tries: sum.tries + row.tries,
                successes: sum.successes + row.successes,
                cost_usd: sum.cost_usd + row.cost_usd,
                subtask_cost_usd: sum.subtask_cost_usd + row.subtask_cost_usd,
                latency_ms: sum.latency_ms.saturating_add(row.latency_ms),
            })
    }

    /// Mean cost per try of `model` at `level`, its subtasks' included,
    /// over every category; `None` before its first try there.
    #[must_use]
    pub fn mean_cost_usd(&self, level: RouteLevel, model: &str) -> Option<f64> {
        let (tries, cost) = self
            .rows
            .iter()
            .filter(|(key, _)| key.level == level && key.model == model)
            .fold((0_u32, 0.0), |(tries, cost), (_, row)| {
                (
                    tries + row.tries,
                    cost + row.cost_usd + row.subtask_cost_usd,
                )
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

    /// Every counted prompt, for [`crate::replay`].
    #[must_use]
    pub fn logged(&self) -> Vec<Logged> {
        self.prompts
            .values()
            .filter_map(|state| {
                let shape = state.shape.as_ref()?;
                let counted = state.counted.as_ref()?;
                Some(Logged {
                    level: shape.level,
                    category: counted.category,
                    difficulty: shape.difficulty,
                    kind: shape.kind.clone(),
                    model: counted.model.clone(),
                    propensity: shape.propensity,
                    success: counted.success?,
                    cost_usd: counted.cost_usd + counted.subtask_cost_usd,
                })
            })
            .collect()
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
            self.shaped(text, level, None, None)
        }

        fn shaped(
            &mut self,
            text: &str,
            level: RouteLevel,
            kind: Option<&str>,
            led: Option<bool>,
        ) -> Hash {
            let mut payload = PromptPayload::new(text);
            payload.category = PromptCategory::Debugging;
            payload.route = Route {
                level,
                ..Route::turn(SONNET, RouteReason::Default)
            };
            payload.kind = kind.map(str::to_string);
            payload.led = led;
            self.push(Transaction::user_prompt_with(self.agent, &payload))
        }

        fn outcome(&mut self, prompt_tx: Hash, model: &str, signal: OutcomeSignal) {
            self.outcome_costing(prompt_tx, model, signal, 0.0);
        }

        fn outcome_costing(
            &mut self,
            prompt_tx: Hash,
            model: &str,
            signal: OutcomeSignal,
            subtask_cost_usd: f64,
        ) {
            let outcome = OutcomePayload {
                prompt_tx,
                category: PromptCategory::Debugging,
                model: model.to_string(),
                success: signal.success(),
                signal,
                cost_usd: 0.25,
                subtask_cost_usd,
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
    fn an_escalated_retry_is_recorded_but_not_counted() {
        let mut log = Log::new();
        let first = log.prompt("fix that part", RouteLevel::Subtask);
        log.outcome(first, SONNET, OutcomeSignal::ChecksFailed);
        let mut retry = PromptPayload::new("fix that part, again");
        retry.category = PromptCategory::Debugging;
        retry.route = Route::subtask(HAIKU, RouteReason::Escalate);
        let retry = log.push(Transaction::user_prompt_with(log.agent, &retry));
        log.outcome(retry, HAIKU, OutcomeSignal::ChecksPassed);

        for stats in [
            GateStats::from_entries(&log.entries),
            GateStats::from_entries(log.entries.iter().rev()),
        ] {
            assert_eq!(debugging(&stats, RouteLevel::Subtask, SONNET), Some((1, 0)));
            assert_eq!(debugging(&stats, RouteLevel::Subtask, HAIKU), None);
            assert_eq!(stats.rows().len(), 1);
        }
    }

    #[test]
    fn a_models_mean_cost_counts_what_its_subtasks_cost() {
        let mut log = Log::new();
        let led = log.shaped("fix it", RouteLevel::Turn, None, Some(true));
        log.outcome_costing(led, SONNET, OutcomeSignal::ChecksPassed, 0.75);
        let stats = GateStats::from_entries(&log.entries);
        assert_eq!(stats.mean_cost_usd(RouteLevel::Turn, SONNET), Some(1.0));
    }

    #[test]
    fn the_replay_log_holds_each_counted_prompt_with_its_propensity() {
        let mut log = Log::new();
        let mut explored = PromptPayload::new("fix the parser");
        explored.category = PromptCategory::Debugging;
        explored.route = Route {
            explored: true,
            propensity: 0.05,
            ..Route::turn(HAIKU, RouteReason::Explore)
        };
        let explored = log.push(Transaction::user_prompt_with(log.agent, &explored));
        log.outcome_costing(explored, HAIKU, OutcomeSignal::ChecksPassed, 0.5);
        let unknown = log.prompt("fix the lexer", RouteLevel::Turn);
        log.outcome(unknown, SONNET, OutcomeSignal::None);
        let mut retry = PromptPayload::new("fix the parser, again");
        retry.route = Route::turn(SONNET, RouteReason::Escalate);
        let retry = log.push(Transaction::user_prompt_with(log.agent, &retry));
        log.outcome(retry, SONNET, OutcomeSignal::ChecksPassed);

        let logged = GateStats::from_entries(&log.entries).logged();
        assert_eq!(
            logged,
            [Logged {
                level: RouteLevel::Turn,
                category: PromptCategory::Debugging,
                difficulty: None,
                kind: None,
                model: HAIKU.to_string(),
                propensity: 0.05,
                success: true,
                cost_usd: 0.75,
            }]
        );
    }

    #[test]
    fn a_failure_the_judge_excuses_counts_for_nothing() {
        let mut log = Log::new();
        let excused = log.prompt("fix the build", RouteLevel::Turn);
        log.outcome(excused, SONNET, OutcomeSignal::ChecksFailed);
        log.outcome(excused, SONNET, OutcomeSignal::JudgedExcused);
        let confirmed = log.prompt("fix the parser", RouteLevel::Turn);
        log.outcome(confirmed, SONNET, OutcomeSignal::ChecksFailed);
        log.outcome(confirmed, SONNET, OutcomeSignal::JudgedFail);
        for stats in [
            GateStats::from_entries(&log.entries),
            GateStats::from_entries(log.entries.iter().rev()),
        ] {
            assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((1, 0)));
        }

        // A later signal still has the last word.
        log.outcome(excused, SONNET, OutcomeSignal::ChecksPassed);
        let stats = GateStats::from_entries(&log.entries);
        assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((2, 1)));

        let mut log = Log::new();
        let only = log.prompt("fix the build", RouteLevel::Turn);
        log.outcome(only, SONNET, OutcomeSignal::ChecksFailed);
        log.outcome(only, SONNET, OutcomeSignal::JudgedExcused);
        let stats = GateStats::from_entries(&log.entries);
        assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), None);
        assert!(stats.rows().is_empty(), "no row is left with zero tries");
    }

    #[test]
    fn a_model_is_ruled_out_once_it_cannot_reach_the_target() {
        let row = |tries, successes| Row {
            tries,
            successes,
            ..Row::default()
        };
        assert!(row(0, 0).can_still_reach(0.85, 10));
        assert!(row(1, 0).can_still_reach(0.85, 10), "9 of 10 would do");
        assert!(!row(2, 0).can_still_reach(0.85, 10), "8 of 10 would not");
        assert!(row(9, 8).can_still_reach(0.85, 10));
        assert!(!row(9, 7).can_still_reach(0.85, 10));
        assert!(row(20, 17).can_still_reach(0.85, 10));
        assert!(!row(20, 16).can_still_reach(0.85, 10));
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

    #[test]
    fn subtask_kinds_learn_apart_from_the_level_row() {
        let mut log = Log::new();
        let rust = log.shaped(
            "fix the borrow",
            RouteLevel::Subtask,
            Some("rust-engineer"),
            None,
        );
        log.outcome(rust, SONNET, OutcomeSignal::ChecksPassed);
        let tests = log.shaped("cover the parser", RouteLevel::Subtask, Some("tests"), None);
        log.outcome(tests, SONNET, OutcomeSignal::ChecksFailed);
        let turn = log.shaped("fix it", RouteLevel::Turn, Some("rust-engineer"), None);
        log.outcome(turn, SONNET, OutcomeSignal::ChecksPassed);

        for stats in [
            GateStats::from_entries(&log.entries),
            GateStats::from_entries(log.entries.iter().rev()),
        ] {
            assert_eq!(debugging(&stats, RouteLevel::Subtask, SONNET), Some((2, 1)));
            let kind = |kind| {
                stats
                    .kind_row(PromptCategory::Debugging, kind, SONNET)
                    .map(|row| (row.tries, row.successes))
            };
            assert_eq!(
                kind("rust-engineer"),
                Some((1, 1)),
                "the turn's kind is not a subtask's"
            );
            assert_eq!(kind("tests"), Some((1, 0)));
            assert_eq!(kind("explorer"), None);
        }
    }

    #[test]
    fn led_and_direct_turns_learn_apart_with_subtask_costs() {
        let mut log = Log::new();
        let led = log.shaped("move the module", RouteLevel::Turn, None, Some(true));
        log.outcome_costing(led, SONNET, OutcomeSignal::ChecksPassed, 1.0);
        let direct = log.shaped("move the other", RouteLevel::Turn, None, Some(false));
        log.outcome(direct, SONNET, OutcomeSignal::ChecksFailed);
        let unsaid = log.prompt("move a third", RouteLevel::Turn);
        log.outcome(unsaid, SONNET, OutcomeSignal::ChecksPassed);
        let mut stats = GateStats::from_entries(&log.entries);

        assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((3, 2)));
        let led_row = stats
            .way_row(PromptCategory::Debugging, SONNET, true)
            .unwrap();
        assert_eq!((led_row.tries, led_row.successes), (1, 1));
        assert_eq!(led_row.mean_total_cost_usd(), Some(1.25));
        let direct_row = stats
            .way_row(PromptCategory::Debugging, SONNET, false)
            .unwrap();
        assert_eq!((direct_row.tries, direct_row.successes), (1, 0));
        assert_eq!(direct_row.mean_total_cost_usd(), Some(0.25));

        log.outcome(led, SONNET, OutcomeSignal::Bad);
        stats.observe(log.entries.last().unwrap());
        let led_row = stats
            .way_row(PromptCategory::Debugging, SONNET, true)
            .unwrap();
        assert_eq!(
            (led_row.tries, led_row.successes),
            (1, 0),
            "replaced, not added"
        );
        assert_eq!(led_row.mean_total_cost_usd(), Some(0.25));
        assert_eq!(
            stats.details().len(),
            3,
            "the unsaid turn is in a row of its own"
        );
    }
}
