//! Filling a table with outcomes, for the tests of this crate and of the
//! crates that build on it (the `test-support` feature).

use aura_core_types::{
    AgentId, OutcomePayload, OutcomeSignal, PromptPayload, RecordEntry, Route, RouteLevel,
    RouteReason, Transaction,
};

use super::GateStats;
use crate::Work;

/// What each outcome [`GateStats::learn`] takes in cost, in USD.
const LEARNED_COST_USD: f64 = 0.01;

/// How long the work of each outcome the table learns took, in
/// milliseconds.
const LEARNED_LATENCY_MS: u64 = 10;

impl GateStats {
    /// Take in `n` outcomes of `work` on `model`, each with its own
    /// prompt (a turn's `led` as given), that all passed (`ok`) or all
    /// failed. Each cost $0.01.
    pub fn learn(
        &mut self,
        work: &Work<'_>,
        model: &str,
        led: Option<bool>,
        outcomes: (usize, bool),
    ) {
        self.learn_costing(work, model, led, outcomes, (LEARNED_COST_USD, 0.0));
    }

    /// [`Self::learn`], each outcome costing `cost_usd` and its subtasks
    /// `subtask_cost_usd`.
    pub fn learn_costing(
        &mut self,
        work: &Work<'_>,
        model: &str,
        led: Option<bool>,
        (n, ok): (usize, bool),
        (cost_usd, subtask_cost_usd): (f64, f64),
    ) {
        let signal = if ok {
            OutcomeSignal::ChecksPassed
        } else {
            OutcomeSignal::ChecksFailed
        };
        for i in 0..n {
            let agent = AgentId::generate();
            let mut prompt = PromptPayload::new(format!("learned {i}"));
            prompt.category = Some(work.category);
            prompt.route = match work.level {
                RouteLevel::Turn => Route::turn(model, RouteReason::Default),
                RouteLevel::Subtask => Route::subtask(model, RouteReason::Default),
            };
            prompt.kind = work.kind.map(str::to_string);
            prompt.led = led;
            prompt.difficulty = work.difficulty;
            let prompt = Transaction::user_prompt_with(agent, &prompt);
            let outcome = OutcomePayload {
                prompt_tx: prompt.hash,
                category: work.category,
                model: model.to_string(),
                success: signal.success(),
                signal,
                cost_usd,
                subtask_cost_usd,
                latency_ms: LEARNED_LATENCY_MS,
            };
            self.observe(&RecordEntry::builder(1, prompt).build());
            self.observe(&RecordEntry::builder(2, Transaction::outcome(agent, &outcome)).build());
        }
    }
}
