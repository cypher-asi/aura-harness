//! Host stats, record imports and seals, and which nodes serve the
//! cloud routes at all.

use super::support::{call, claim, cloud_state, NODE_TOKEN};
use crate::config::NodeConfig;
use crate::gateway::RouterState;
use aura_core_types::{AgentId, RecordEntry, Transaction};
use axum::http::StatusCode;

#[tokio::test]
async fn an_unclaimed_node_refuses_and_a_claimed_one_serves_its_hub() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cloud_state(dir.path());
    let (status, _) = call(&state, "GET", "/v1/host", Some(NODE_TOKEN), None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    // The node's own startup token is never accepted on a cloud node.
    let (status, _) = call(&state, "GET", "/v1/host", Some("test"), None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    claim(&state);
    let (status, _) = call(&state, "GET", "/v1/host", Some("test"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = call(&state, "GET", "/v1/host", Some(NODE_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["cpu_count"].as_u64().unwrap_or(0) > 0, "{body}");
    let (status, _) = call(&state, "GET", "/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
}

async fn post_import(
    state: &RouterState,
    import: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    call(
        state,
        "POST",
        "/v1/sync/import",
        Some(NODE_TOKEN),
        Some(import),
    )
    .await
}

#[tokio::test]
async fn import_appends_verified_entries_and_seal_stops_further_writes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cloud_state(dir.path());
    claim(&state);
    let agent = AgentId::new([5; 32]);
    let entries: Vec<RecordEntry> = (1..=3)
        .map(|seq| {
            RecordEntry::builder(seq, Transaction::user_prompt(agent, format!("p{seq}"))).build()
        })
        .collect();
    let import = serde_json::json!({
        "agent_id": agent.to_hex(),
        "name": "re36",
        "entries": entries,
        "done": true,
    });
    let (status, body) = post_import(&state, import.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["head_seq"], 3);
    // Importing the same page again changes nothing.
    let (status, body) = post_import(&state, import.clone()).await;
    assert_eq!(
        (status, body["head_seq"].as_u64()),
        (StatusCode::OK, Some(3))
    );

    let (status, body) = call(
        &state,
        "POST",
        &format!("/v1/sync/seal/{}", agent.to_hex()),
        Some(NODE_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["head_seq"], 3);
    let (status, _) = post_import(&state, import).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let tx =
        serde_json::json!({"agent_id": agent.to_hex(), "kind": "user_prompt", "payload": "aGk="});
    let (status, _) = call(&state, "POST", "/tx", Some(NODE_TOKEN), Some(tx)).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_gap_in_an_import_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cloud_state(dir.path());
    claim(&state);
    let agent = AgentId::new([6; 32]);
    let entry = RecordEntry::builder(2, Transaction::user_prompt(agent, "p2")).build();
    let import = serde_json::json!({"agent_id": agent.to_hex(), "name": "x", "entries": [entry], "done": true});
    let (status, body) = call(
        &state,
        "POST",
        "/v1/sync/import",
        Some(NODE_TOKEN),
        Some(import),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
}

#[tokio::test]
async fn other_nodes_do_not_serve_cloud_routes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut state = cloud_state(dir.path());
    state.cloud = crate::cloud_node::CloudNode::new(&NodeConfig::default());
    state.config.require_auth = false;
    state.config.cloud_node = false;
    let (status, _) = call(&state, "GET", "/v1/host", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
