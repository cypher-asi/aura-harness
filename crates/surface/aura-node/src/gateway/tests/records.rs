use super::support::*;
use crate::gateway::*;
use aura_core_types::AgentId;
use axum::body::Body;
use axum::http::StatusCode;
use tower::util::ServiceExt;

#[tokio::test]
async fn test_get_head_new_agent() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let agent_id = AgentId::generate();
    let req = authed_request()
        .uri(format!("/agents/{}/head", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["head_seq"], 0);
}

#[tokio::test]
async fn test_get_head_invalid_agent_id() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let req = authed_request()
        .uri("/agents/zzz-bad/head")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_scan_record_empty() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let agent_id = AgentId::generate();
    let req = authed_request()
        .uri(format!("/agents/{}/record", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_scan_record_with_query_params() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let agent_id = AgentId::generate();
    let req = authed_request()
        .uri(format!(
            "/agents/{}/record?from_seq=5&limit=10",
            agent_id.to_hex()
        ))
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_scan_record_invalid_agent() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let req = authed_request()
        .uri("/agents/bad-hex/record")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_submit_tx_all_kinds() {
    let kinds = [
        "user_prompt",
        "agent_msg",
        "trigger",
        "action_result",
        "system",
    ];

    for kind in kinds {
        let store = create_test_store();
        let state = test_router_state(store);
        let app = create_router(state);

        let agent_id = AgentId::generate();
        let payload_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("payload for {kind}"),
        );

        let body = serde_json::json!({
            "agent_id": agent_id.to_hex(),
            "kind": kind,
            "payload": payload_b64
        });

        let req = authed_request()
            .method("POST")
            .uri("/tx")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::ACCEPTED,
            "kind '{kind}' should be accepted"
        );
    }
}

#[tokio::test]
async fn test_nonexistent_route_returns_404() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let req = authed_request()
        .uri("/nonexistent")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
