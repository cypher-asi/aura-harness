use super::support::*;
use crate::gateway::*;
use aura_core_types::AgentId;
use axum::body::Body;
use axum::http::StatusCode;
use tower::util::ServiceExt;

// ============================================================================
// Memory Events
// ============================================================================

#[tokio::test]
async fn test_memory_create_and_list_events() {
    let state = test_router_state_with_managers();
    let agent_id = AgentId::generate();
    let app = create_router(state);

    let body = serde_json::json!({
        "event_type": "task_run",
        "summary": "completed build"
    });
    let req = authed_request()
        .method("POST")
        .uri(format!("/memory/{}/events", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = authed_request()
        .uri(format!("/memory/{}/events", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let events: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["event_type"], "task_run");
}

#[tokio::test]
async fn test_memory_bulk_delete_events_alias() {
    let state = test_router_state_with_managers();
    let agent_id = AgentId::generate();
    let app = create_router(state);

    let body = serde_json::json!({
        "event_type": "task_run",
        "summary": "completed build"
    });
    let req = authed_request()
        .method("POST")
        .uri(format!("/memory/{}/events", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let _ = app.clone().oneshot(req).await.unwrap();

    // Phase B / Commit 3 follow-up: `delete_events_before` truncates
    // the cutoff to millisecond resolution before comparing with the
    // event key, so the test must guarantee `before > event.timestamp`
    // at that resolution. The pre-Phase-B run was lucky to fall on a
    // millisecond boundary every time; the engine extraction
    // re-ordered the test schedule enough that the cutoff and the
    // event sometimes landed in the same millisecond and the deletion
    // saw zero matches. A 5ms sleep is comfortably above Windows'
    // ~1ms timer resolution and adds nothing meaningful to the suite
    // wall clock.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let bulk_delete_body = serde_json::json!({
        "before": chrono::Utc::now().to_rfc3339()
    });
    let req = authed_request()
        .method("POST")
        .uri(format!(
            "/api/agents/{}/memory/events/bulk-delete",
            agent_id.to_hex()
        ))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&bulk_delete_body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result["deleted"], 1);

    let req = authed_request()
        .uri(format!("/memory/{}/events", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let events: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert!(events.is_empty());
}

// ============================================================================
// Memory Procedures
// ============================================================================

#[tokio::test]
async fn test_memory_create_and_list_procedures() {
    let state = test_router_state_with_managers();
    let agent_id = AgentId::generate();
    let app = create_router(state);

    let body = serde_json::json!({
        "name": "deploy",
        "trigger": "user says deploy",
        "steps": ["cargo build", "cargo test", "deploy binary"]
    });
    let req = authed_request()
        .method("POST")
        .uri(format!("/memory/{}/procedures", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = authed_request()
        .uri(format!("/memory/{}/procedures", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let procs: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(procs.len(), 1);
    assert_eq!(procs[0]["name"], "deploy");
}

// ============================================================================
// Memory Stats & Wipe
// ============================================================================

#[tokio::test]
async fn test_memory_stats() {
    let state = test_router_state_with_managers();
    let agent_id = AgentId::generate();
    let app = create_router(state);

    let req = authed_request()
        .uri(format!("/memory/{}/stats", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(stats["facts"], 0);
    assert_eq!(stats["events"], 0);
    assert_eq!(stats["procedures"], 0);
}

#[tokio::test]
async fn test_memory_wipe() {
    let state = test_router_state_with_managers();
    let agent_id = AgentId::generate();
    let app = create_router(state);

    let body = serde_json::json!({ "key": "k", "value": "v" });
    let req = authed_request()
        .method("POST")
        .uri(format!("/memory/{}/facts", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let _ = app.clone().oneshot(req).await.unwrap();

    let req = authed_request()
        .method("POST")
        .uri(format!("/memory/{}/wipe", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let req = authed_request()
        .uri(format!("/memory/{}/stats", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(stats["facts"], 0);
    assert_eq!(stats["events"], 0);
}

#[tokio::test]
async fn test_memory_snapshot() {
    let state = test_router_state_with_managers();
    let agent_id = AgentId::generate();
    let app = create_router(state);

    let body = serde_json::json!({ "key": "lang", "value": "Rust" });
    let req = authed_request()
        .method("POST")
        .uri(format!("/memory/{}/facts", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let _ = app.clone().oneshot(req).await.unwrap();

    let req = authed_request()
        .uri(format!("/memory/{}/snapshot", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let snapshot: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(snapshot["facts"].as_array().unwrap().len(), 1);
    assert!(snapshot["events"].as_array().unwrap().is_empty());
    assert!(snapshot["procedures"].as_array().unwrap().is_empty());
}

// ============================================================================
// Memory — invalid agent id
// ============================================================================

#[tokio::test]
async fn test_memory_invalid_agent_id() {
    let state = test_router_state_with_managers();
    let app = create_router(state);

    let req = authed_request()
        .uri("/memory/bad-hex/facts")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ============================================================================
// Memory — 503 when not configured
// ============================================================================

#[tokio::test]
async fn test_memory_returns_503_when_not_configured() {
    let store = create_test_store();
    let state = test_router_state(store);
    let agent_id = AgentId::generate();
    let app = create_router(state);

    let req = authed_request()
        .uri(format!("/memory/{}/facts", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}
