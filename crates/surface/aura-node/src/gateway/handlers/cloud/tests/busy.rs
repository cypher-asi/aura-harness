//! One turn at a time per agent: `POST /v1/run` turns away a chat run of
//! an agent with a turn under way in another run.

use super::support::{call, claim, cloud_state, NODE_TOKEN};
use super::watch::run_of;
use aura_core_protocol::{
    AgentCapabilities, AgentIdentity, AgentPermissionsWire, ModelSelection, RuntimeRequest,
    RuntimeRequestType, WorkspaceLocation,
};
use aura_core_types::AgentId;
use axum::http::StatusCode;

/// What the hub asks for to start a chat run of `agent`.
fn chat_run_of(agent: AgentId) -> serde_json::Value {
    let request = RuntimeRequest {
        r#type: RuntimeRequestType::Chat {
            conversation_messages: Vec::new(),
        },
        agent_identity: AgentIdentity {
            partition_id: Some(agent.to_hex()),
            ..AgentIdentity::default()
        },
        model: ModelSelection::default(),
        workspace: WorkspaceLocation::default(),
        project: None,
        agent_permissions: AgentPermissionsWire::default(),
        tool_permissions: None,
        agent_capabilities: AgentCapabilities::default(),
        auth_jwt: None,
        user_id: "hub".to_string(),
        mode: None,
    };
    serde_json::to_value(request).expect("request")
}

#[tokio::test]
async fn a_chat_run_of_an_agent_with_a_turn_under_way_is_turned_away() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cloud_state(dir.path());
    claim(&state);
    let agent = AgentId::new([1; 32]);
    let other = AgentId::new([2; 32]);
    // A run a message started on its own, which the hub never saw.
    let busy = run_of(&state, "r1", agent);
    busy.activity.begin("Re-review the parser".into());

    let (status, body) = call(
        &state,
        "POST",
        "/v1/run",
        Some(NODE_TOKEN),
        Some(chat_run_of(agent)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "agent_busy");
    assert_eq!(body["run_id"], "r1");
    assert_eq!(state.chat_runs.len(), 1, "no second run of it");

    // Other agents are not held up, nor is this one once its turn ends.
    let (status, body) = call(
        &state,
        "POST",
        "/v1/run",
        Some(NODE_TOKEN),
        Some(chat_run_of(other)),
    )
    .await;
    assert_ne!(status, StatusCode::CONFLICT, "{body}");
    busy.activity.end();
    let (status, body) = call(
        &state,
        "POST",
        "/v1/run",
        Some(NODE_TOKEN),
        Some(chat_run_of(agent)),
    )
    .await;
    assert_ne!(status, StatusCode::CONFLICT, "{body}");
}
