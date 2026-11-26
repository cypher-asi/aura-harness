use super::support::*;
use crate::gateway::*;
use aura_core_types::AgentId;
use axum::body::Body;
use axum::http::StatusCode;
use tower::util::ServiceExt;

// ============================================================================
// Skills
// ============================================================================

#[tokio::test]
async fn test_skills_list() {
    let state = test_router_state_with_managers();
    let app = create_router(state);

    let req = authed_request()
        .uri("/api/skills")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let skills: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert!(skills.is_empty() || skills.iter().all(|s| s["name"].is_string()));
}

#[tokio::test]
async fn test_skills_get_not_found() {
    let state = test_router_state_with_managers();
    let app = create_router(state);

    let req = authed_request()
        .uri("/api/skills/nonexistent")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_skills_agent_install_and_list() {
    let state = test_router_state_with_managers();
    let agent_id = AgentId::generate();
    let app = create_router(state);

    let body = serde_json::json!({ "name": "test-skill" });
    let req = authed_request()
        .method("POST")
        .uri(format!("/api/agents/{}/skills", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = authed_request()
        .uri(format!("/api/agents/{}/skills", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let installs: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(installs.len(), 1);
    assert_eq!(installs[0]["skill_name"], "test-skill");
}

#[tokio::test]
async fn test_skills_agent_uninstall() {
    let state = test_router_state_with_managers();
    let agent_id = AgentId::generate();
    let app = create_router(state);

    let body = serde_json::json!({ "name": "removable" });
    let req = authed_request()
        .method("POST")
        .uri(format!("/api/agents/{}/skills", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = authed_request()
        .method("DELETE")
        .uri(format!(
            "/api/agents/{}/skills/removable",
            agent_id.to_hex()
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let req = authed_request()
        .uri(format!("/api/agents/{}/skills", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let installs: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert!(installs.is_empty());
}

#[tokio::test]
async fn test_skills_legacy_harness_aliases() {
    let state = test_router_state_with_managers();
    let agent_id = AgentId::generate();
    let app = create_router(state);

    let body = serde_json::json!({ "name": "legacy-skill" });
    let req = authed_request()
        .method("POST")
        .uri(format!("/api/harness/agents/{}/skills", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = authed_request()
        .uri(format!("/api/harness/agents/{}/skills", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let installs: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(installs.len(), 1);
    assert_eq!(installs[0]["skill_name"], "legacy-skill");

    let req = authed_request()
        .method("DELETE")
        .uri(format!(
            "/api/harness/agents/{}/skills/legacy-skill",
            agent_id.to_hex()
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let req = authed_request()
        .uri(format!("/api/agents/{}/skills", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let installs: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert!(installs.is_empty());
}

#[tokio::test]
async fn test_skills_returns_503_when_not_configured() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let req = authed_request()
        .uri("/api/skills")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}
