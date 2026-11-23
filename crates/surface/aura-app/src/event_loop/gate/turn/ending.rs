//! What a turn Gate labelled is recorded with once it ends: the `Outcome`
//! of the prompt its last attempt answers, and that attempt's line in
//! Gate's attempt log when Gate routed the turn.

use super::attempts::{Asked, Current};
use super::elapsed_ms;
use crate::event_loop::agent_events::ForwarderState;
use crate::event_loop::agents::AgentSession;
use crate::event_loop::gate::followup::ended_signal;
use crate::event_loop::subagents::AgentHub;
use crate::event_loop::turns::TurnParts;
use aura_agent::{AgentError, AgentLoopResult};
use aura_core_types::{OutcomePayload, OutcomeSignal, PromptCategory};
use aura_gate::attempts::Terms;
use aura_gate::evidence::Evidence;
use aura_gate::live;
use std::sync::Arc;

/// How a turn Gate labelled ended, for recording its outcome once the
/// turn's result is handled.
pub(crate) struct Ending {
    /// The last attempt
    pub(super) last: Current,
    pub(super) category: PromptCategory,
    /// When the turn started, in Unix milliseconds
    pub(super) started_ms: u64,
    /// What the turn ran under, for Gate's attempt log
    pub(super) terms: Terms,
    /// For what the turn's subtasks cost and its attempt log, when Gate
    /// routed the turn
    pub(super) hub: Option<Arc<AgentHub>>,
    /// What the last attempt's runs before its final one cost: those on
    /// routes that refused the user's sign-in, and those Gate's follow-up
    /// had carry on
    pub(super) cost_usd: f64,
    /// What the last attempt ended with once Gate's follow-up had its say;
    /// `None` when Gate did not follow it up
    pub(super) signal: Option<OutcomeSignal>,
    /// The files the last attempt changed by any means, when the harness
    /// compared the workspace before and after
    pub(super) changed: Option<Vec<String>>,
}

impl Ending {
    /// The ending of the turn of `asked` whose last attempt is `last`,
    /// Gate routing it through `hub` when it does, before anything else
    /// is known of it.
    pub(super) fn new(asked: &Asked<'_>, last: Current, hub: Option<Arc<AgentHub>>) -> Self {
        Self {
            last,
            category: asked.category,
            started_ms: asked.started_ms,
            terms: asked.terms(),
            hub,
            cost_usd: 0.0,
            signal: None,
            changed: None,
        }
    }

    /// The outcome the turn ends with, read from `result`, how its last
    /// attempt ended, before the result is handed on: cancelled when the
    /// user cancelled the turn or its route refused the user's sign-in
    /// (`stopped`), else what Gate's follow-up said, else what the
    /// attempt says of itself, else unavailable.
    pub(crate) fn settle(
        self,
        result: &Result<AgentLoopResult, AgentError>,
        stopped: bool,
    ) -> Settled {
        let turn_start = self.last.turn_start;
        let signal = if stopped {
            OutcomeSignal::Cancelled
        } else {
            (self.signal)
                .or_else(|| ended_signal(result, turn_start))
                .unwrap_or(OutcomeSignal::Unavailable)
        };
        let left = (result.as_ref().ok())
            .filter(|_| self.hub.is_some())
            .map(|result| live::evidence_of(result, turn_start, self.changed.as_deref()));
        Settled {
            ending: self,
            signal,
            left,
        }
    }
}

/// The outcome of a turn Gate labelled, read and about to be recorded.
pub(crate) struct Settled {
    ending: Ending,
    pub(super) signal: OutcomeSignal,
    /// What the last attempt left, for Gate's attempt log
    pub(super) left: Option<Evidence>,
}

impl Settled {
    /// Record the outcome on `session` as the work of the model `turn`
    /// last ran on, its last run costing what `forwarded` counted, and log
    /// the attempt when Gate routed the turn.
    pub(crate) async fn record(
        self,
        session: &AgentSession,
        turn: &TurnParts<'_>,
        forwarded: &ForwarderState,
    ) {
        let Self {
            ending,
            signal,
            left,
        } = self;
        let agent = session.agent_id();
        let outcome = OutcomePayload {
            prompt_tx: ending.last.prompt_tx,
            category: ending.category,
            model: turn.agent_loop.config().model.clone(),
            success: signal.success(),
            signal,
            cost_usd: ending.cost_usd + forwarded.usage.cost_usd,
            subtask_cost_usd: (ending.hub.as_ref())
                .map_or(0.0, |hub| hub.subtask_cost_usd(agent, ending.started_ms)),
            latency_ms: elapsed_ms(ending.last.started),
        };
        if let Some(hub) = &ending.hub {
            hub.gate.log_attempt(agent, &outcome, ending.terms, left);
        }
        live::record(&session.kernel, agent, &outcome).await;
    }
}
