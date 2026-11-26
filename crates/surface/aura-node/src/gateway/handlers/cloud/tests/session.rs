//! Session checkpoints the hub puts and gets, `/new`, and the trace runs
//! it copies back.

use super::support::{call, claim, cloud_state, import, NODE_TOKEN};
use crate::gateway::RouterState;
use aura_agent::checkpoint::{SessionCheckpoint, CHECKPOINT_VERSION};
use aura_core_types::AgentId;
use axum::http::StatusCode;

fn checkpoint(record_seq: u64) -> SessionCheckpoint {
    SessionCheckpoint::new(
        1_000,
        record_seq,
        vec![
            aura_model::Message::user("p1"),
            aura_model::Message::assistant("a1"),
        ],
        Vec::new(),
        aura_agent::checkpoint::SessionUsage::default(),
    )
}

fn json(checkpoint: &SessionCheckpoint) -> serde_json::Value {
    serde_json::to_value(checkpoint).expect("json")
}

/// `PUT /v1/sync/session/:agent_id` of `checkpoint`, with the node token.
async fn put_session(
    state: &RouterState,
    agent: AgentId,
    checkpoint: &SessionCheckpoint,
) -> (StatusCode, serde_json::Value) {
    let uri = format!("/v1/sync/session/{}", agent.to_hex());
    call(state, "PUT", &uri, Some(NODE_TOKEN), Some(json(checkpoint))).await
}

/// Seal `agent` on the node, as the hub does when it takes the agent back.
async fn seal(state: &RouterState, agent: AgentId) {
    let uri = format!("/v1/sync/seal/{}", agent.to_hex());
    call(state, "POST", &uri, Some(NODE_TOKEN), None).await;
}

/// The session the node's next run for `agent` starts with.
async fn next_run(state: &RouterState, agent: AgentId) -> crate::gateway::session::Session {
    let workspace = state.config.workspaces_path().join("agent");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let request = aura_core_protocol::RuntimeRequest {
        r#type: aura_core_protocol::RuntimeRequestType::Chat {
            conversation_messages: Vec::new(),
        },
        agent_identity: aura_core_protocol::AgentIdentity {
            partition_id: Some(agent.to_hex()),
            ..aura_core_protocol::AgentIdentity::default()
        },
        model: aura_core_protocol::ModelSelection::default(),
        workspace: aura_core_protocol::WorkspaceLocation {
            workspace: Some(workspace.display().to_string()),
            ..aura_core_protocol::WorkspaceLocation::default()
        },
        project: None,
        agent_permissions: aura_core_protocol::AgentPermissionsWire::default(),
        tool_permissions: None,
        agent_capabilities: aura_core_protocol::AgentCapabilities::default(),
        auth_jwt: None,
        user_id: "hub".to_string(),
        mode: None,
    };
    let ctx = crate::gateway::session::WsContext::from_state(state, None);
    crate::gateway::session::prepare_chat_session(request, &ctx)
        .await
        .expect("prepare")
}

#[tokio::test]
async fn a_session_is_taken_up_to_the_record_and_never_once_sealed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cloud_state(dir.path());
    claim(&state);
    let agent = AgentId::new([9; 32]);
    let uri = format!("/v1/sync/session/{}", agent.to_hex());
    import(&state, agent, 2).await;

    let (status, body) = put_session(&state, agent, &checkpoint(3)).await;
    assert_eq!(status, StatusCode::CONFLICT, "past the record: {body}");
    let mut unknown = checkpoint(2);
    unknown.version = CHECKPOINT_VERSION + 1;
    let (status, _) = put_session(&state, agent, &unknown).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(&state, "PUT", &uri, None, Some(json(&checkpoint(2)))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = put_session(&state, agent, &checkpoint(2)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (body["record_seq"].as_u64(), body["messages"].as_u64()),
        (Some(2), Some(2))
    );
    let session = next_run(&state, agent).await;
    assert!(session.continues);
    assert_eq!(session.started_ms, 1_000);
    assert_eq!(session.messages.len(), 2);

    seal(&state, agent).await;
    let (status, _) = put_session(&state, agent, &checkpoint(2)).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_new_session_starts_on_the_node() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cloud_state(dir.path());
    claim(&state);
    let agent = AgentId::new([10; 32]);
    let uri = format!("/v1/sync/session/{}/new", agent.to_hex());
    import(&state, agent, 2).await;
    let (status, _) = put_session(&state, agent, &checkpoint(2)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call(&state, "POST", &uri, Some(NODE_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["head_seq"], 3);
    let entries = state.store.scan_record(agent, 3, 1).expect("scan");
    assert_eq!(
        entries[0].tx.tx_type,
        aura_core_types::TransactionType::SessionStart
    );
    let session = next_run(&state, agent).await;
    assert!(
        session.continues,
        "the node wrote the session start already"
    );
    assert!(session.messages.is_empty(), "{:?}", session.messages);

    seal(&state, agent).await;
    let (status, _) = call(&state, "POST", &uri, Some(NODE_TOKEN), None).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn the_hub_gets_the_session_and_the_runs_back() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cloud_state(dir.path());
    claim(&state);
    let agent = AgentId::new([12; 32]);
    let uri = format!("/v1/sync/session/{}", agent.to_hex());
    let (status, body) = call(&state, "GET", &uri, Some(NODE_TOKEN), None).await;
    assert_eq!((status, body), (StatusCode::OK, serde_json::Value::Null));
    import(&state, agent, 2).await;
    let (status, _) = put_session(&state, agent, &checkpoint(2)).await;
    assert_eq!(status, StatusCode::OK);

    let traces = |who: AgentId| format!("/v1/sync/traces?agent={}", who.to_hex());
    let run = state
        .cloud
        .trace(
            agent,
            aura_agent_trace::RunMeta::new("m", aura_agent_trace::RunSource::Gateway),
        )
        .expect("run");
    run.begin_turn("p1");
    let (status, body) = call(&state, "GET", &traces(agent), Some(NODE_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let runs = body["runs"].as_array().cloned().unwrap_or_default();
    assert_eq!(runs.len(), 1, "{body}");
    assert_eq!(runs[0]["meta"]["agent_id"], agent.to_hex());
    assert_eq!(
        runs[0]["steps"].as_array().map(Vec::len),
        Some(1),
        "the open run is flushed first"
    );
    let other = traces(AgentId::new([13; 32]));
    let (_, body) = call(&state, "GET", &other, Some(NODE_TOKEN), None).await;
    assert_eq!(body["runs"], serde_json::json!([]));

    seal(&state, agent).await;
    let (status, body) = call(&state, "GET", &uri, Some(NODE_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "served after a seal: {body}");
    assert_eq!(body["record_seq"], 2);
    let (_, body) = call(&state, "GET", &traces(agent), Some(NODE_TOKEN), None).await;
    assert!(
        body["runs"][0]["summary"].is_object(),
        "the sealed run is finished: {body}"
    );
}
