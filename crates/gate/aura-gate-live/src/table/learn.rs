//! Filling the table by hand, for tests.

use super::{lock, GateLive};
use aura_core_types::{
    AgentId, Difficulty, OutcomePayload, OutcomeSignal, PromptCategory, PromptPayload, RecordEntry,
    Route, RouteLevel, RouteReason, Transaction,
};

impl GateLive {
    /// Take in `n` outcomes of work at `level` asking for `category` on
    /// `model`, the classifier having judged it of `difficulty` when that
    /// is given, each with its own prompt (a turn's `led` as given), that
    /// all passed or all failed.
    pub fn learn(
        &self,
        level: RouteLevel,
        (category, difficulty): (PromptCategory, Option<Difficulty>),
        model: &str,
        led: Option<bool>,
        (n, ok): (usize, bool),
    ) {
        let signal = if ok {
            OutcomeSignal::ChecksPassed
        } else {
            OutcomeSignal::ChecksFailed
        };
        let mut stats = lock(&self.stats);
        for i in 0..n {
            let agent = AgentId::generate();
            let mut prompt = PromptPayload::new(format!("learned {i}"));
            prompt.category = Some(category);
            prompt.route = match level {
                RouteLevel::Turn => Route::turn(model, RouteReason::Default),
                RouteLevel::Subtask => Route::subtask(model, RouteReason::Default),
            };
            prompt.led = led;
            prompt.difficulty = difficulty;
            let prompt = Transaction::user_prompt_with(agent, &prompt);
            let outcome = OutcomePayload {
                prompt_tx: prompt.hash,
                category,
                model: model.to_string(),
                success: signal.success(),
                signal,
                cost_usd: 0.01,
                subtask_cost_usd: 0.0,
                latency_ms: 10,
            };
            stats.observe(&RecordEntry::builder(1, prompt).build());
            stats.observe(&RecordEntry::builder(2, Transaction::outcome(agent, &outcome)).build());
        }
    }
}
