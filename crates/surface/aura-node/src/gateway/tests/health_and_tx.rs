use super::support::*;
use crate::gateway::*;
use aura_core_types::AgentId;
use aura_exec_tools::ToolConfig;
use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use tower::util::ServiceExt;

#[tokio::test]
async fn test_health_endpoint() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "/health must remain reachable without a bearer token"
    );

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
    assert!(json["version"].is_string());
    // git_sha is baked in via AURA_HARNESS_GIT_SHA in container images and
    // null for local builds; either way the key must be present.
    assert!(
        json.get("git_sha").is_some(),
        "/health must always include the git_sha key"
    );
    assert!(json["git_sha"].is_string() || json["git_sha"].is_null());
    assert_eq!(json["safe_workspace"], true);
}

/// Verify that `/health` exposes the effective tool policy so the
/// `aura-os-desktop` `--external-harness` startup check can detect a
/// misconfigured external harness (the 3.0-class `run_command`
/// regression) without any authenticated probe. The test builds a
/// `RouterState` with a permissive `ToolConfig` and asserts each
/// policy field lands on the response with the same value.
#[tokio::test]
async fn test_health_endpoint_exposes_tool_policy_for_external_harness_probe() {
    let store = create_test_store();
    let mut state = test_router_state(store);
    state.tool_config = ToolConfig {
        command: aura_exec_tools::CommandPolicy {
            enabled: true,
            command_allowlist: vec!["cargo".into(), "git".into()],
            binary_allowlist: vec!["cargo".into(), "git".into()],
            allow_shell: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let app = create_router(state);

    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(
        json["run_command_enabled"], true,
        "external-harness probe relies on this field"
    );
    assert_eq!(json["shell_enabled"], true);
    assert!(json["allowed_commands"].is_array());
    let allowed: Vec<String> = json["allowed_commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(allowed, vec!["cargo", "git"]);
    assert!(json["binary_allowlist"].is_array());
    let binaries: Vec<String> = json["binary_allowlist"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(binaries, vec!["cargo", "git"]);
}

/// Symmetric negative test: a `ToolConfig::default()` — the
/// `aura-exec-tools` crate's kernel-level fail-closed baseline — must still
/// report `run_command_enabled=false` over `/health` so operators who
/// deliberately wire the minimal executor can observe the narrowed
/// execution surface.
#[tokio::test]
async fn test_health_endpoint_reports_run_command_disabled_on_default_tool_config() {
    let store = create_test_store();
    let state = test_router_state(store); // ToolConfig::default()
    let app = create_router(state);

    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["run_command_enabled"], false,
        "aura-exec-tools ToolConfig::default() is the kernel-level fail-closed baseline"
    );
    assert_eq!(json["shell_enabled"], false);
    assert!(
        json["binary_allowlist"].as_array().unwrap().is_empty(),
        "default ToolConfig should report an empty binary allowlist"
    );
}

#[tokio::test]
async fn test_submit_tx_valid() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let agent_id = AgentId::generate();
    let payload_b64 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "Hello agent");

    let body = serde_json::json!({
        "agent_id": agent_id.to_hex(),
        "kind": "user_prompt",
        "payload": payload_b64
    });

    let req = authed_request()
        .method("POST")
        .uri("/tx")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["accepted"].as_bool().unwrap());
    assert!(json["tx_id"].is_string());
}

#[tokio::test]
async fn test_submit_tx_invalid_agent_id() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let body = serde_json::json!({
        "agent_id": "not-hex",
        "kind": "user_prompt",
        "payload": "aGVsbG8="
    });

    let req = authed_request()
        .method("POST")
        .uri("/tx")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_submit_tx_invalid_kind() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let agent_id = AgentId::generate();
    let body = serde_json::json!({
        "agent_id": agent_id.to_hex(),
        "kind": "invalid_kind",
        "payload": "aGVsbG8="
    });

    let req = authed_request()
        .method("POST")
        .uri("/tx")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_submit_tx_rejects_kernel_only_kinds() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let agent_id = AgentId::generate();
    for kind in ["policy_decision", "tool_execution", "tool_proposal"] {
        let body = serde_json::json!({
            "agent_id": agent_id.to_hex(),
            "kind": kind,
            "payload": "e30=",
        });
        let req = authed_request()
            .method("POST")
            .uri("/tx")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "kind {kind}");
    }
}

#[tokio::test]
async fn test_tool_call_audit_returns_proposal_decision_and_execution() {
    let store = create_test_store();
    let agent_id = AgentId::generate();
    append_tool_call_chain(&store, agent_id, 1, "tu-other");
    let (proposal_hash, decision_hash, execution_hash) =
        append_tool_call_chain(&store, agent_id, 4, "tu-1");
    let app = create_router(test_router_state(store));

    let req = authed_request()
        .uri(format!(
            "/agents/{}/tool_calls/tu-1/audit",
            agent_id.to_hex()
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["proposal"]["tx"]["hash"],
        serde_json::to_value(proposal_hash).unwrap()
    );
    assert_eq!(json["proposal"]["seq"], 4);
    assert_eq!(
        json["policy_decision"]["tx"]["hash"],
        serde_json::to_value(decision_hash).unwrap()
    );
    assert_eq!(json["decision"]["decided_by"]["source"], "user_default");
    assert_eq!(json["decision"]["decided_by"]["rule"], "full_access");
    assert_eq!(
        json["execution"]["tx"]["hash"],
        serde_json::to_value(execution_hash).unwrap()
    );

    let missing = authed_request()
        .uri(format!(
            "/agents/{}/tool_calls/nope/audit",
            agent_id.to_hex()
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(missing).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_submit_tx_invalid_base64() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let agent_id = AgentId::generate();
    let body = serde_json::json!({
        "agent_id": agent_id.to_hex(),
        "kind": "user_prompt",
        "payload": "!!! not base64 !!!"
    });

    let req = authed_request()
        .method("POST")
        .uri("/tx")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_submit_tx_rejects_mid_session_permissions_change() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let agent_id = AgentId::generate();
    let payload_json = serde_json::json!({
        "kind": "agent_permissions",
        "capabilities": [{"type": "spawnAgent"}]
    });
    let payload_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        serde_json::to_vec(&payload_json).unwrap(),
    );
    let body = serde_json::json!({
        "agent_id": agent_id.to_hex(),
        "kind": "system",
        "payload": payload_b64,
    });

    let req = authed_request()
        .method("POST")
        .uri("/tx")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body).to_string();
    assert!(text.contains("permissions:"), "got: {text}");
    assert!(text.contains("frozen"), "got: {text}");
}

#[tokio::test]
async fn test_submit_tx_allows_normal_system_payload() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let agent_id = AgentId::generate();
    let payload = serde_json::json!({"kind": "identity", "name": "agent-x"});
    let payload_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        serde_json::to_vec(&payload).unwrap(),
    );
    let body = serde_json::json!({
        "agent_id": agent_id.to_hex(),
        "kind": "system",
        "payload": payload_b64,
    });

    let req = authed_request()
        .method("POST")
        .uri("/tx")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
}
