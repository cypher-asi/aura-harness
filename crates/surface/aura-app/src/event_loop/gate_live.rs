//! Gate's table while the app runs, shared by every agent's turns.
//!
//! [`GateLive`] holds the [`GateStats`] Gate routes by. It is rebuilt
//! from the record at startup ([`GateLive::rebuild_in_background`]) and,
//! as every kernel's record observer, takes in each prompt and `Outcome`
//! as it is committed, so the next routing decision already sees it.
//! Startup scan and live entries may overlap; [`GateStats`] counts each
//! prompt once whatever the order.
//!
//! It also keeps the latest routing decisions for `/gate`, and the model
//! each agent's routed work is on, which decides what the subtasks it
//! dispatches are routed among ([`aura_agent_gate::below_lead`]).

use aura_agent_gate::{Candidate, Decision, GateStats, SplitMix64};
use aura_agent_kernel::RecordObserver;
use aura_config::GateConfig;
use aura_core_types::{AgentId, PromptCategory, RecordEntry, RouteLevel, RouteReason};
use aura_store_db::ReadStore;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tracing::{info, warn};

/// Routing decisions `/gate` lists.
pub(crate) const RECENT_DECISIONS: usize = 20;

/// Record entries read per store scan, and per lock of the table while
/// rebuilding.
const SCAN_BATCH: usize = 500;

/// A routing decision, as `/gate` lists it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RecentRoute {
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
    stats: Mutex<GateStats>,
    rng: Mutex<SplitMix64>,
    recent: Mutex<VecDeque<RecentRoute>>,
    /// The model Gate sent each agent's work in flight to
    working_on: Mutex<HashMap<AgentId, String>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
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
    pub(crate) fn with_rng(settings: GateConfig, rng: SplitMix64) -> Self {
        Self {
            settings,
            stats: Mutex::new(GateStats::default()),
            rng: Mutex::new(rng),
            recent: Mutex::new(VecDeque::with_capacity(RECENT_DECISIONS)),
            working_on: Mutex::new(HashMap::new()),
        }
    }

    /// Note that Gate sent the work `agent` is starting to `model`.
    pub(crate) fn note_model(&self, agent: AgentId, model: &str) {
        lock(&self.working_on).insert(agent, model.to_string());
    }

    /// The model Gate sent `agent`'s work in flight to; `None` when it
    /// has none or Gate did not route it.
    pub(crate) fn model_of(&self, agent: AgentId) -> Option<String> {
        lock(&self.working_on).get(&agent).cloned()
    }

    /// The work of `agent` ended.
    pub(crate) fn forget_model(&self, agent: AgentId) {
        lock(&self.working_on).remove(&agent);
    }

    /// The settings Gate routes with.
    pub(crate) const fn settings(&self) -> &GateConfig {
        &self.settings
    }

    /// A copy of the table as it is now.
    pub(crate) fn stats(&self) -> GateStats {
        lock(&self.stats).clone()
    }

    /// Gate's decision for work at `level` asking for `category`, among
    /// `candidates`, remembered for `/gate`. `None` when there are no
    /// candidates.
    pub(crate) fn decide(
        &self,
        level: RouteLevel,
        category: PromptCategory,
        candidates: &[Candidate],
    ) -> Option<Decision> {
        self.decide_with(level, category, candidates, &self.settings)
    }

    /// [`Self::decide`] with no exploring: where work goes again when the
    /// model first chosen can't take it.
    pub(crate) fn redecide(
        &self,
        level: RouteLevel,
        category: PromptCategory,
        candidates: &[Candidate],
    ) -> Option<Decision> {
        let settings = GateConfig {
            explore_rate: 0.0,
            ..self.settings
        };
        self.decide_with(level, category, candidates, &settings)
    }

    fn decide_with(
        &self,
        level: RouteLevel,
        category: PromptCategory,
        candidates: &[Candidate],
        settings: &GateConfig,
    ) -> Option<Decision> {
        let decision = {
            let stats = lock(&self.stats);
            let mut rng = lock(&self.rng);
            aura_agent_gate::pick(&stats, level, category, candidates, settings, &mut *rng)?
        };
        self.remember(RecentRoute {
            ts_ms: super::processes::now_ms(),
            level,
            category,
            model: decision.model.route_label(),
            reason: decision.reason,
        });
        Some(decision)
    }

    /// Where work at `level` asking for `category`, whose attempt on
    /// `failed` failed, goes next among `candidates` (see
    /// [`aura_agent_gate::escalate`]), remembered for `/gate`. `None`
    /// when no candidate is stronger.
    pub(crate) fn escalate(
        &self,
        level: RouteLevel,
        category: PromptCategory,
        failed: &str,
        candidates: &[Candidate],
    ) -> Option<Decision> {
        let decision = {
            let stats = lock(&self.stats);
            aura_agent_gate::escalate(&stats, level, failed, candidates)?
        };
        self.remember(RecentRoute {
            ts_ms: super::processes::now_ms(),
            level,
            category,
            model: decision.model.route_label(),
            reason: decision.reason,
        });
        Some(decision)
    }

    fn remember(&self, route: RecentRoute) {
        let mut recent = lock(&self.recent);
        if recent.len() == RECENT_DECISIONS {
            recent.pop_front();
        }
        recent.push_back(route);
    }

    /// The latest decisions, oldest first.
    pub(crate) fn recent(&self) -> Vec<RecentRoute> {
        lock(&self.recent).iter().cloned().collect()
    }

    /// Take in the records of `agents` in `store`, oldest first. Returns
    /// how many entries were read.
    pub(crate) fn rebuild<S: ReadStore + ?Sized>(&self, store: &S, agents: &[AgentId]) -> usize {
        let mut read = 0;
        for &agent_id in agents {
            let mut from = 1;
            loop {
                let entries = match store.scan_record(agent_id, from, SCAN_BATCH) {
                    Ok(entries) => entries,
                    Err(e) => {
                        warn!(%agent_id, error = %e, "Gate could not read a record");
                        break;
                    }
                };
                let Some(last) = entries.last().map(|e| e.seq) else {
                    break;
                };
                {
                    let mut stats = lock(&self.stats);
                    for entry in &entries {
                        stats.observe(entry);
                    }
                }
                read += entries.len();
                if entries.len() < SCAN_BATCH {
                    break;
                }
                from = last + 1;
            }
        }
        read
    }

    /// [`Self::rebuild`] on a blocking thread, so startup doesn't wait
    /// for the scan. Without a Tokio runtime it runs here.
    pub(crate) fn rebuild_in_background(
        self: &Arc<Self>,
        store: Arc<dyn ReadStore>,
        agents: Vec<AgentId>,
    ) {
        let live = Arc::clone(self);
        let scan = move || {
            let read = live.rebuild(store.as_ref(), &agents);
            info!(
                agents = agents.len(),
                entries = read,
                "Gate rebuilt its table"
            );
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(scan);
            }
            Err(_) => scan(),
        }
    }
}

impl RecordObserver for GateLive {
    fn on_entry(&self, entry: &RecordEntry) {
        use aura_core_types::TransactionType;
        if matches!(
            entry.tx.tx_type,
            TransactionType::UserPrompt | TransactionType::Outcome
        ) {
            lock(&self.stats).observe(entry);
        }
    }
}

#[cfg(test)]
mod tests;
