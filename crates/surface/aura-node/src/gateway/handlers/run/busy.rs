//! One turn at a time per agent: a chat run asked for while another run
//! of the same agent has a turn under way is turned away.

use crate::gateway::RouterState;
use aura_core_protocol::RuntimeRequest;
use axum::http::StatusCode;
use axum::Json;

/// Turn away a chat run for an agent that has a turn under way in
/// another run (one a message started on its own, say): `409` with code
/// `agent_busy` and that run's id. The caller waits for it to end
/// instead of running the agent twice at once.
pub(super) fn refuse_busy_agent(
    state: &RouterState,
    req: &RuntimeRequest,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let Some(agent) = req
        .agent_identity
        .partition_id
        .as_deref()
        .and_then(|id| aura_core_types::AgentId::from_hex(id).ok())
    else {
        return Ok(());
    };
    match crate::gateway::session::busy_run_of(&state.chat_runs, agent) {
        Some(run_id) => Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": format!("agent {agent} has a turn under way in run {run_id}"),
                "code": "agent_busy",
                "run_id": run_id,
            })),
        )),
        None => Ok(()),
    }
}
