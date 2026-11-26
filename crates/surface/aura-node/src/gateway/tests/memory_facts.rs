use super::memory_support::*;
use super::support::*;
use crate::gateway::*;
use aura_context_memory::{MemoryAccessContext, MemoryQueryContext};
use aura_core_types::AgentId;
use axum::body::Body;
use axum::http::StatusCode;
use tower::util::ServiceExt;

// ============================================================================
// Memory Facts
// ============================================================================

#[tokio::test]
async fn test_memory_create_and_list_facts() {
    let state = test_router_state_with_managers();
    let agent_id = AgentId::generate();
    let app = create_router(state);

    let body = serde_json::json!({
        "key": "language",
        "value": "Rust",
        "confidence": 0.9,
        "importance": 0.7
    });
    let req = authed_request()
        .method("POST")
        .uri(format!("/memory/{}/facts", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = authed_request()
        .uri(format!("/memory/{}/facts", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let facts: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0]["key"], "language");
}

#[tokio::test]
async fn test_memory_get_fact_by_key() {
    let state = test_router_state_with_managers();
    let agent_id = AgentId::generate();
    let app = create_router(state);

    let body = serde_json::json!({ "key": "framework", "value": "Axum" });
    let req = authed_request()
        .method("POST")
        .uri(format!("/memory/{}/facts", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = authed_request()
        .uri(format!(
            "/memory/{}/facts/by-key/framework",
            agent_id.to_hex()
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let fact: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(fact["key"], "framework");
    assert_eq!(fact["value"], "Axum");
}

#[tokio::test]
async fn test_memory_delete_fact() {
    let state = test_router_state_with_managers();
    let agent_id = AgentId::generate();
    let app = create_router(state);

    let body = serde_json::json!({ "key": "temp", "value": "delete me" });
    let req = authed_request()
        .method("POST")
        .uri(format!("/memory/{}/facts", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let fact: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let fact_id = fact["fact_id"].as_str().unwrap();

    let req = authed_request()
        .method("DELETE")
        .uri(format!("/memory/{}/facts/{}", agent_id.to_hex(), fact_id))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let req = authed_request()
        .uri(format!("/memory/{}/facts", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let facts: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert!(facts.is_empty());
}

/// Seed facts across agent, project and user scopes for two agents, plus an
/// unscoped legacy fact for `agent_b`.
async fn seed_partitioned_facts(app: &axum::Router, agent_a: AgentId, agent_b: AgentId) {
    for (agent_id, key, scope) in [
        (agent_a, "agent_a_private", "agent"),
        (agent_a, "project_shared", "project"),
        (agent_a, "user_personal", "user"),
        (agent_b, "agent_b_private", "agent"),
    ] {
        let body = scoped_fact_body(key, key, scope);
        let uri = format!("/memory/{}/facts?{PROJECT_X_QUERY}", agent_id.to_hex());
        assert_eq!(post_json(app, &uri, &body).await.status(), StatusCode::OK);
    }

    let legacy = serde_json::json!({ "key": "legacy_unscoped", "value": "legacy" });
    let uri = format!("/memory/{}/facts", agent_b.to_hex());
    assert_eq!(post_json(app, &uri, &legacy).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_memory_api_enforces_project_agent_project_and_user_partitions() {
    let state = test_router_state_with_managers();
    let agent_a = AgentId::generate();
    let agent_b = AgentId::generate();
    let app = create_router(state);
    seed_partitioned_facts(&app, agent_a, agent_b).await;

    let uri = format!("/memory/{}/snapshot?{PROJECT_X_QUERY}", agent_b.to_hex());
    let response = get(&app, &uri).await;
    assert_eq!(response.status(), StatusCode::OK);
    let packet = json_body(response).await;
    let keys: std::collections::HashSet<_> = fact_keys(&packet).into_iter().collect();
    assert_eq!(
        keys,
        std::collections::HashSet::from(["agent_b_private", "project_shared", "user_personal"])
    );

    let uri = format!("/memory/{}/snapshot?{PROJECT_Y_QUERY}", agent_b.to_hex());
    let packet = json_body(get(&app, &uri).await).await;
    assert_eq!(fact_keys(&packet), vec!["user_personal"]);

    let uri = format!(
        "/memory/{}/snapshot?{PROJECT_X_QUERY}&include_legacy=true",
        agent_b.to_hex()
    );
    let packet = json_body(get(&app, &uri).await).await;
    assert!(packet["facts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|fact| fact["key"] == "legacy_unscoped"));

    let conflicting_personal =
        scoped_fact_body("user_personal", "contradictory preference", "user");
    let uri = format!("/memory/{}/facts?{PROJECT_X_QUERY}", agent_b.to_hex());
    let response = post_json(&app, &uri, &conflicting_personal).await;
    assert_eq!(response.status(), StatusCode::OK);
    let fact = json_body(response).await;
    assert_eq!(fact["continuity"]["status"], "pending");
}

#[tokio::test]
async fn test_memory_continuity_config_roundtrip_and_empty_trace() {
    let state = test_router_state_with_managers();
    let agent_id = AgentId::generate();
    let app = create_router(state);
    let continuity_uri = format!("/api/agents/{}/memory/continuity", agent_id.to_hex());

    let resp = get(&app, &continuity_uri).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let default_config = json_body(resp).await;
    assert_eq!(default_config["use_memory"], true);
    assert_eq!(default_config["retrieval_mode"], "query_aware");

    let config = serde_json::json!({
        "use_memory": true,
        "generate_memory": true,
        "write_policy": "approval",
        "retrieval_mode": "query_aware",
        "allow_user_scope": false,
        "allow_project_scope": false
    });
    let resp = send_json(&app, "PUT", &continuity_uri, &config).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let persisted = json_body(get(&app, &continuity_uri).await).await;
    assert_eq!(persisted["write_policy"], "approval");

    let uri = format!("/api/agents/{}/memory/retrieval/latest", agent_id.to_hex());
    let resp = get(&app, &uri).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await, serde_json::Value::Null);
}

#[tokio::test]
async fn test_memory_retrieval_trace_is_scoped_to_project_agent_partition() {
    let state = test_router_state_with_managers();
    let manager = state.memory_manager.as_ref().unwrap().clone();
    let agent_id = AgentId::generate();
    let app = create_router(state);

    let body = scoped_fact_body("release_train", "Tuesday", "project");
    let uri = format!("/memory/{}/facts?{PROJECT_X_QUERY}", agent_id.to_hex());
    assert_eq!(post_json(&app, &uri, &body).await.status(), StatusCode::OK);

    let mut prompt = String::new();
    manager
        .prepare_context_with_query(
            agent_id,
            &mut prompt,
            MemoryQueryContext {
                text: "When is the release train?".to_string(),
                access: MemoryAccessContext {
                    project_id: Some("project-x".to_string()),
                    user_id: Some("user-u".to_string()),
                    include_legacy: false,
                },
                ..MemoryQueryContext::default()
            },
        )
        .await;
    assert!(prompt.contains("Tuesday"));

    let uri = format!(
        "/api/agents/{}/memory/retrieval/latest?{PROJECT_X_QUERY}",
        agent_id.to_hex()
    );
    let trace = json_body(get(&app, &uri).await).await;
    assert_eq!(trace["selected_count"], 1);

    let uri = format!(
        "/api/agents/{}/memory/retrieval/latest?{PROJECT_Y_QUERY}",
        agent_id.to_hex()
    );
    assert_eq!(
        json_body(get(&app, &uri).await).await,
        serde_json::Value::Null
    );
}
