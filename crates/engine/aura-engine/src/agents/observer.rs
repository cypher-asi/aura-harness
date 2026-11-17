//! [`TurnObserver`]: what a surface hears about hosted agents' turns,
//! for recording them.

use aura_agent::{AgentLoopEvent, AgentLoopResult};
use aura_core_types::{AgentId, SubagentExit};

/// A hosted agent's turn that is about to run.
#[derive(Debug, Clone, Copy)]
pub struct HostedTurn<'a> {
    pub agent_id: AgentId,
    /// The agent that spawned it.
    pub parent: Option<AgentId>,
    pub name: &'a str,
    pub model: &'a str,
    /// What the agent is asked, as it sees it.
    pub prompt: &'a str,
}

/// Hears about every hosted agent's turns. Calls for one turn come in
/// order: [`Self::turn_started`], each loop event, then
/// [`Self::turn_ended`]. Implementations must not block for long.
pub trait TurnObserver: Send + Sync {
    /// A turn starts.
    fn turn_started(&self, turn: &HostedTurn<'_>);
    /// One loop event of the running turn of `agent_id`.
    fn on_event(&self, agent_id: AgentId, event: &AgentLoopEvent);
    /// The turn ended with `exit`; `result` is `None` when it stopped
    /// before the loop produced one.
    fn turn_ended(&self, agent_id: AgentId, result: Option<&AgentLoopResult>, exit: &SubagentExit);
    /// `agent_id` was removed from the host. Called with the host's
    /// agent table locked.
    fn agent_removed(&self, agent_id: AgentId);
}
