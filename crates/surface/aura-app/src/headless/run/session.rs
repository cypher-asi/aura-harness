//! The session a run continues: its id, which `--resume` takes, the
//! session opened as the run asks, and the line that announces it.

use super::{Headless, SessionChoice};
use crate::agent_registry::RegisteredAgent;
use crate::event_loop::handlers::{mark_session_start, resume_checkpoint};
use crate::event_loop::subagents::lock;
use crate::event_loop::AgentSession;
use crate::headless::output::{Output, StreamEvent};
use aura_core_types::AgentId;
use tracing::info;

/// `<agent hex>:<session start ms>`: names one saved session.
pub(super) fn session_id(agent_id: AgentId, started_ms: u64) -> String {
    format!("{}:{started_ms}", agent_id.to_hex())
}

/// The agent and session start named by `id`. A bare number names a
/// session of `default_agent`.
pub(super) fn parse_session_id(id: &str, default_agent: AgentId) -> Result<(AgentId, u64), String> {
    let id = id.trim();
    let (agent, started) = match id.rsplit_once(':') {
        Some((agent, started)) => (
            AgentId::from_hex(agent).map_err(|e| format!("bad agent in session id `{id}`: {e}"))?,
            started,
        ),
        None => (default_agent, id),
    };
    let started = started
        .parse::<u64>()
        .map_err(|_| format!("`{id}` is not a session id (expected <agent>:<number>)"))?;
    Ok((agent, started))
}

impl Headless {
    /// Open the session `choice` names. Returns it and whether it
    /// continues an earlier one.
    pub(super) async fn open(
        &self,
        choice: &SessionChoice,
    ) -> Result<(AgentSession, bool), String> {
        let (agent, started_ms) = self.agent_for(choice)?;
        let folder = lock(&self.hub.registry).project_folder(agent.agent_id());
        let mut session = self
            .hub
            .factory
            .open(&agent, Vec::new(), folder)
            .map_err(|e| format!("opening {}: {e:#}", agent.name))?;

        match (choice, started_ms) {
            (SessionChoice::New, _) => {
                if !session.messages.is_empty() {
                    mark_session_start(&session).await;
                }
                session.reset();
                Ok((session, false))
            }
            (SessionChoice::Resume(id), Some(started_ms)) if started_ms != session.started_ms => {
                let checkpoint = session
                    .checkpoints
                    .as_ref()
                    .and_then(|c| c.load(session.agent_id(), started_ms))
                    .ok_or_else(|| format!("no saved session {id}"))?;
                info!(session = %id, "Resuming a saved session");
                resume_checkpoint(&mut session, checkpoint).await;
                Ok((session, true))
            }
            _ => {
                let resumed = !session.messages.is_empty();
                Ok((session, resumed))
            }
        }
    }

    /// The agent whose session `choice` names, and that session's start
    /// when it names a saved one.
    fn agent_for(&self, choice: &SessionChoice) -> Result<(RegisteredAgent, Option<u64>), String> {
        let headless = lock(&self.hub.registry)
            .headless()
            .map_err(|e| format!("registering the headless agent: {e:#}"))?;
        let SessionChoice::Resume(id) = choice else {
            return Ok((headless, None));
        };
        let (agent_id, started_ms) = parse_session_id(id, headless.agent_id())?;
        let agent = lock(&self.hub.registry)
            .find(agent_id)
            .cloned()
            .ok_or_else(|| format!("no agent for session {id}"))?;
        Ok((agent, Some(started_ms)))
    }

    /// Tell `output` the run is on `session`, which continues an earlier
    /// one when `resumed`, and what its agent may use.
    pub(super) fn announce(&self, output: &Output, session: &AgentSession, resumed: bool) {
        let workspace = session.kernel.agent_workspace();
        // A turn that leads has fewer tools; which turns do is only known
        // once their prompt is classified.
        let (_, tools) = self.hub.turn(session.definition.as_deref(), &workspace);
        let tools = self.tool_filter.filter(tools);
        let session_id = session_id(session.agent_id(), session.started_ms);
        let agent_hex = session.agent_id().to_hex();
        let cwd = self.workspace_root.display().to_string();
        let mode = format!("{:?}", self.mode).to_lowercase();
        output.event(&StreamEvent::Init {
            session_id: &session_id,
            agent_id: &agent_hex,
            model: &self.model,
            cwd: &cwd,
            mode: &mode,
            tools: tools.iter().map(|t| t.name.as_str()).collect(),
            resumed,
        });
    }
}
