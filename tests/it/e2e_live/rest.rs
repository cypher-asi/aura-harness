//! Suites 1-2: health, REST API and WebSocket session bootstrap.

use std::time::Duration;

use aura_core_types::AgentId;
use serde_json::{json, Value};

use crate::common::{
    chat_request_payload, http_client, post_runtime_request, TestServer, WsClient,
};

// ============================================================================
// Suite 1: Health and REST API
// ============================================================================

#[tokio::test]

async fn test_health() {
    let server = TestServer::start().await;
    let resp = http_client()
        .get(format!("{}/health", server.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["version"].is_string());
}

/// When `AURA_NODE_REQUIRE_AUTH` is off (the default), the router
/// should not attach `require_bearer_mw` and callers without a Bearer
/// header must reach handler logic rather than getting bounced at the
/// middleware layer. Asserting "not 401" is stronger than asserting a
/// specific success status — the test fixture wires up no automaton
/// controller, so `/v1/run/list` legitimately replies `503`; that's
/// still proof the request traversed the protected sub-router without
/// being rejected for missing auth (which is the regression this guard
/// actually cares about).
#[tokio::test]
async fn test_protected_route_open_when_auth_disabled() {
    let server = TestServer::start_without_auth().await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let resp = client
        .get(format!("{}/v1/run/list", server.base_url()))
        .send()
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "protected route should not return 401 when require_auth=false"
    );
}

#[tokio::test]

async fn test_submit_tx() {
    let server = TestServer::start().await;
    let agent_id = AgentId::generate();
    let payload_b64 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "Hello from e2e");
    let body = json!({
        "agent_id": agent_id.to_hex(),
        "kind": "user_prompt",
        "payload": payload_b64
    });
    let resp = http_client()
        .post(format!("{}/tx", server.base_url()))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);
    let data: Value = resp.json().await.unwrap();
    assert!(data["accepted"].as_bool().unwrap());
    assert!(data["tx_id"].is_string());
}

#[tokio::test]

async fn test_get_head() {
    let server = TestServer::start().await;
    let agent_id = AgentId::generate();
    let resp = http_client()
        .get(format!(
            "{}/agents/{}/head",
            server.base_url(),
            agent_id.to_hex()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let data: Value = resp.json().await.unwrap();
    assert_eq!(data["head_seq"], 0);
}

#[tokio::test]

async fn test_scan_record() {
    let server = TestServer::start().await;
    let agent_id = AgentId::generate();
    let resp = http_client()
        .get(format!(
            "{}/agents/{}/record?from_seq=1&limit=10",
            server.base_url(),
            agent_id.to_hex()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let data: Value = resp.json().await.unwrap();
    assert!(data.as_array().unwrap().is_empty());
}

// ============================================================================
// Suite 2: WebSocket Session Lifecycle (Phase A wire shape)
// ============================================================================

#[tokio::test]
async fn test_ws_runtime_request_bootstrap() {
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("test-agent");
    std::fs::create_dir_all(&ws_path).unwrap();

    // Use the raw POST + connect pattern so we can introspect the
    // SessionReady frame (the high-level `open_chat_run` helper
    // consumes it).
    let payload = chat_request_payload(&ws_path, None);
    let resp = post_runtime_request(&server, &payload).await;
    assert_eq!(resp.status(), 201);
    let body: Value = resp.json().await.unwrap();
    let run_id = body["run_id"].as_str().unwrap().to_string();
    let mut ws = WsClient::connect(&server.stream_url(&run_id)).await;
    let ready = ws.expect_session_ready().await;

    let tools = ready["tools"].as_array().unwrap();
    let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    assert!(tool_names.contains(&"list_files"), "missing list_files");
    assert!(tool_names.contains(&"read_file"), "missing read_file");
    assert!(tool_names.contains(&"write_file"), "missing write_file");
    assert!(tool_names.contains(&"edit_file"), "missing edit_file");
    assert!(tool_names.contains(&"delete_file"), "missing delete_file");
    assert!(tool_names.contains(&"find_files"), "missing find_files");
    assert!(tool_names.contains(&"search_code"), "missing search_code");
    assert!(tool_names.contains(&"run_command"), "missing run_command");
    assert!(tool_names.contains(&"stat_file"), "missing stat_file");
}
