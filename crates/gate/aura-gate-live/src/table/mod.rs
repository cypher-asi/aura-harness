//! [`GateLive`]: Gate's table while the app runs, shared by every agent's
//! turns.
//!
//! It holds the [`GateStats`] Gate routes by. It is rebuilt from the
//! record at startup ([`GateLive::rebuild_in_background`]) and, as every
//! kernel's record observer, takes in each prompt and `Outcome` as it is
//! committed, so the next routing decision already sees it. Startup scan
//! and live entries may overlap; [`GateStats`] counts each prompt once
//! whatever the order.
//!
//! It also keeps the latest routing decisions for `/gate`, and the model
//! each agent's routed work is on, which decides what the subtasks it
//! dispatches are routed among ([`aura_gate_route::below_lead`]).
//!
//! When it is given an attempt log ([`GateLive::log_attempts_to`]), each
//! attempt and what later proves it right or wrong is appended there.
//! What the log says each sign of failure has earned
//! ([`GateLive::tiers`]) decides which signs have work reviewed before it
//! stands.

use crate::runtime::now_ms;
use aura_config::GateConfig;
use aura_core_types::{
    AgentId, Difficulty, OutcomeSignal, PromptCategory, RouteLevel, RouteReason,
};
use aura_gate_attempts::{AttemptLog, Tiers};
use aura_gate_lead::Way;
use aura_gate_route::stats::GateStats;
use aura_gate_route::{Candidate, Decision, Random, SplitMix64, Work};
use aura_model::ThinkingEffort;
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

#[cfg(any(test, feature = "test-support"))]
mod learn;
#[cfg(test)]
mod tests;

/// Routing decisions `/gate` lists.
pub(crate) const RECENT_DECISIONS: usize = 20;

/// A routing decision, as `/gate` lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct RecentRoute {
    /// When it was made, in Unix milliseconds
    pub ts_ms: u64,
    pub level: RouteLevel,
    pub category: PromptCategory,
    /// The model and who serves it, e.g. `claude-sonnet-4-6 (claude plan)`
    pub model: String,
    pub reason: RouteReason,
}

/// Gate's table and recent decisions; see the module docs.
#[derive(Debug)]
pub struct GateLive {
    settings: GateConfig,
    pub(crate) stats: Mutex<GateStats>,
    rng: Mutex<SplitMix64>,
    recent: Mutex<VecDeque<RecentRoute>>,
    /// The model Gate sent each agent's work in flight to
    working_on: Mutex<HashMap<AgentId, String>>,
    /// Where attempts are logged, once set
    pub(crate) log: OnceLock<AttemptLog>,
    /// What each sign of failure has earned in the attempt log, as last
    /// read
    pub(crate) tiers: Mutex<Tiers>,
}

/// Lock `mutex`, taking it over from a thread that panicked holding it:
/// every update leaves the table whole.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Default for GateLive {
    fn default() -> Self {
        Self::new(GateConfig::defaults())
    }
}

impl GateLive {
    /// An empty table routing with `settings`.
    #[must_use]
    pub fn new(settings: GateConfig) -> Self {
        Self::with_rng(settings, SplitMix64::from_clock())
    }

    /// [`Self::new`] drawing from `rng`, for repeatable exploration.
    #[must_use]
    pub fn with_rng(settings: GateConfig, rng: SplitMix64) -> Self {
        Self {
            settings,
            stats: Mutex::new(GateStats::default()),
            rng: Mutex::new(rng),
            recent: Mutex::new(VecDeque::with_capacity(RECENT_DECISIONS)),
            working_on: Mutex::new(HashMap::new()),
            log: OnceLock::new(),
            tiers: Mutex::new(Tiers::default()),
        }
    }

    /// What each sign of failure has earned in the attempt log, as last
    /// read ([`Self::learn_tiers`]); nothing before that.
    pub fn tiers(&self) -> Tiers {
        lock(&self.tiers).clone()
    }

    /// Note that Gate sent the work `agent` is starting to `model`.
    pub fn note_model(&self, agent: AgentId, model: &str) {
        lock(&self.working_on).insert(agent, model.to_string());
    }

    /// The model Gate sent `agent`'s work in flight to; `None` when it
    /// has none or Gate did not route it.
    pub fn model_of(&self, agent: AgentId) -> Option<String> {
        lock(&self.working_on).get(&agent).cloned()
    }

    /// The work of `agent` ended.
    pub fn forget_model(&self, agent: AgentId) {
        lock(&self.working_on).remove(&agent);
    }

    /// The settings Gate routes with.
    #[must_use]
    pub const fn settings(&self) -> &GateConfig {
        &self.settings
    }

    /// Whether a draw falls in a sample of `share` of all draws. A share
    /// of 0 or 1 takes no draw, which leaves exploration's draws as they
    /// were.
    pub fn sampled(&self, share: f64) -> bool {
        if share >= 1.0 {
            return true;
        }
        share > 0.0 && lock(&self.rng).unit() < share
    }

    /// A copy of the table as it is now.
    pub fn stats(&self) -> GateStats {
        lock(&self.stats).clone()
    }

    /// What `f` makes of the table as it is now, without copying it. The
    /// table stays locked while `f` runs, so routing and the record
    /// observers wait for it.
    pub fn with_stats<T>(&self, f: impl FnOnce(&GateStats) -> T) -> T {
        f(&lock(&self.stats))
    }

    /// Gate's decision for `work` among `candidates`, remembered for
    /// `/gate`. `None` when there are no candidates.
    pub fn decide(&self, work: &Work<'_>, candidates: &[Candidate]) -> Option<Decision> {
        self.decide_with(work, candidates, &self.settings)
    }

    /// [`Self::decide`] with no exploring: where work goes again when the
    /// model first chosen can't take it.
    pub fn redecide(&self, work: &Work<'_>, candidates: &[Candidate]) -> Option<Decision> {
        self.decide_with(work, candidates, &self.settings.without_exploring())
    }

    fn decide_with(
        &self,
        work: &Work<'_>,
        candidates: &[Candidate],
        settings: &GateConfig,
    ) -> Option<Decision> {
        let decision = {
            let stats = lock(&self.stats);
            let mut rng = lock(&self.rng);
            aura_gate_route::pick(&stats, work, candidates, settings, &mut *rng)?
        };
        self.remember(work.level, work.category, &decision);
        Some(decision)
    }

    /// Where work at `level` asking for `category`, whose attempt on
    /// `failed` (a model and the effort Gate chose for it) ended with
    /// `signal`, goes next among `candidates` (see
    /// [`aura_gate_route::escalate`]), remembered for `/gate`. `None`
    /// when nothing is left to try.
    pub fn escalate(
        &self,
        level: RouteLevel,
        category: PromptCategory,
        failed: (&str, Option<ThinkingEffort>),
        signal: OutcomeSignal,
        candidates: &[Candidate],
    ) -> Option<Decision> {
        let decision = {
            let stats = lock(&self.stats);
            aura_gate_route::escalate(&stats, level, failed, signal, candidates)?
        };
        self.remember(level, category, &decision);
        Some(decision)
    }

    /// Whether a turn asking for `category` of `difficulty` on `model` is
    /// led (see [`aura_gate_lead::choose_way`]); it explores the other
    /// way only when `explore`.
    pub fn way(
        &self,
        work: (PromptCategory, Option<Difficulty>),
        model: &str,
        explore: bool,
    ) -> Way {
        let settings = if explore {
            self.settings
        } else {
            self.settings.without_exploring()
        };
        let stats = lock(&self.stats);
        let mut rng = lock(&self.rng);
        aura_gate_lead::choose_way(&stats, work, model, &settings, &mut *rng)
    }

    fn remember(&self, level: RouteLevel, category: PromptCategory, decision: &Decision) {
        let route = RecentRoute {
            ts_ms: now_ms(),
            level,
            category,
            model: decision.model.route_label(),
            reason: decision.reason,
        };
        let mut recent = lock(&self.recent);
        if recent.len() == RECENT_DECISIONS {
            recent.pop_front();
        }
        recent.push_back(route);
    }

    /// The latest decisions, oldest first.
    pub fn recent(&self) -> Vec<RecentRoute> {
        lock(&self.recent).iter().cloned().collect()
    }
}
