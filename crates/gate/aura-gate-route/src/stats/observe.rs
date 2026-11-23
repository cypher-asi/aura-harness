//! Taking record entries into the table: what each prompt says of its
//! work, and which of its outcomes it counts with.

use aura_core_types::{
    AgentId, Difficulty, Hash, OutcomePayload, PromptCategory, PromptPayload, RecordEntry,
    RouteLevel, RouteReason,
};

use super::GateStats;

/// What a prompt says about how its work was done.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Shape {
    pub(super) level: RouteLevel,
    /// The permanent agent a subtask went to.
    pub(super) kind: Option<String>,
    /// Whether a turn was led; `None` when its prompt does not say.
    pub(super) led: Option<bool>,
    pub(super) difficulty: Option<Difficulty>,
    /// How likely the router was to pick its model.
    pub(super) propensity: f64,
}

impl Shape {
    fn of(prompt: PromptPayload) -> Self {
        let level = prompt.route.level;
        Self {
            level,
            kind: prompt.kind.filter(|_| level == RouteLevel::Subtask),
            led: prompt.led.filter(|_| level == RouteLevel::Turn),
            difficulty: prompt.difficulty,
            propensity: prompt.route.propensity,
        }
    }
}

/// The outcome a prompt is counted with.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Counted {
    /// Record sequence number of the outcome.
    seq: u64,
    pub(super) category: PromptCategory,
    pub(super) model: String,
    /// The outcome takes back what was said before: the prompt then
    /// counts in no row.
    pub(super) void: bool,
    /// Whether the work succeeded; `None` when it ended with nothing
    /// that says, so that it counts for its cost only.
    pub(super) success: Option<bool>,
    pub(super) cost_usd: f64,
    pub(super) subtask_cost_usd: f64,
    pub(super) latency_ms: u64,
}

impl Counted {
    fn from_outcome(seq: u64, outcome: OutcomePayload) -> Self {
        let void = outcome.signal.voids();
        Self {
            seq,
            category: outcome.category,
            void,
            success: outcome.success.filter(|_| !void),
            model: outcome.model,
            cost_usd: outcome.cost_usd,
            subtask_cost_usd: outcome.subtask_cost_usd,
            latency_ms: outcome.latency_ms,
        }
    }

    /// Whether the outcome says how the work went, or takes that back.
    const fn decides(&self) -> bool {
        self.void || self.success.is_some()
    }

    /// Whether `self` counts instead of `other` for the same prompt: one
    /// that says how the work went over one that does not, else the later
    /// one.
    fn supersedes(&self, other: &Self) -> bool {
        if self.decides() == other.decides() {
            self.seq > other.seq
        } else {
            self.decides()
        }
    }
}

/// What is known about one prompt.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct PromptState {
    /// From the prompt, once it has been seen.
    pub(super) shape: Option<Shape>,
    /// The prompt is an escalated retry, which is never counted.
    ignored: bool,
    /// The outcome it counts with, once the work has ended.
    pub(super) counted: Option<Counted>,
}

impl GateStats {
    /// Take `entry` into account: a prompt says the level of its route,
    /// and an outcome with a known result may become what its prompt
    /// counts with. Other entries are ignored.
    pub fn observe(&mut self, entry: &RecordEntry) {
        let agent = entry.tx.agent_id;
        if let Some(prompt) = entry.tx.prompt_payload() {
            let key = (agent, entry.tx.hash);
            if prompt.route.reason == RouteReason::Escalate {
                self.prompts.entry(key).or_default().ignored = true;
            } else {
                self.observe_prompt(key, Shape::of(prompt));
            }
        } else if let Some(outcome) = entry.tx.outcome_payload() {
            self.observe_outcome(agent, entry.seq, outcome);
        }
    }

    /// The first sight of a prompt counts the outcome that came before it.
    fn observe_prompt(&mut self, prompt: (AgentId, Hash), shape: Shape) {
        let state = self.prompts.entry(prompt).or_default();
        if state.shape.is_some() {
            return;
        }
        if let Some(counted) = &state.counted {
            self.table.add(counted, &shape);
        }
        state.shape = Some(shape);
    }

    /// An outcome that supersedes its prompt's takes its place, in the
    /// table too once the prompt has been seen.
    fn observe_outcome(&mut self, agent: AgentId, seq: u64, outcome: OutcomePayload) {
        let state = self.prompts.entry((agent, outcome.prompt_tx)).or_default();
        let new = Counted::from_outcome(seq, outcome);
        if state.ignored
            || state
                .counted
                .as_ref()
                .is_some_and(|old| !new.supersedes(old))
        {
            return;
        }
        let old = state.counted.replace(new);
        if let (Some(shape), Some(new)) = (&state.shape, &state.counted) {
            if let Some(old) = &old {
                self.table.remove(old, shape);
            }
            self.table.add(new, shape);
        }
    }
}
