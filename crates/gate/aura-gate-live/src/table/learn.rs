//! Filling the table by hand, for tests.

use super::{lock, GateLive};
use aura_core_types::{Difficulty, PromptCategory, RouteLevel};
use aura_gate_route::Work;

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
        outcomes: (usize, bool),
    ) {
        let work = Work::new(level, category, None).of(difficulty);
        lock(&self.stats).learn(&work, model, led, outcomes);
    }
}
