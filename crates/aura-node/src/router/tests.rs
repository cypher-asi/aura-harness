use super::*;
use aura_agent::KernelModelGateway;
use aura_core::AgentId;
use aura_kernel::{ExecutorRouter, Kernel, KernelConfig};
use aura_memory::{
    ConsolidationConfig, MemoryManager, ProcedureConfig, RefinerConfig, RetrievalConfig,
    WriteConfig,
};
use aura_reasoner::MockProvider;
use aura_skills::{SkillInstallStore, SkillLoader, SkillManager};
use aura_store::RocksStore;
use axum::body::Body;
use axum::http::Request;
use tower::util::ServiceExt;

fn test_router_state(store: Arc<dyn Store>) -> RouterState {
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("mock"));
    let scheduler = Arc::new(Scheduler::new(
        store.clone(),
        provider.clone(),
        vec![],
        vec![],
        std::path::PathBuf::from("/tmp/workspaces"),
        None,
    ));
    RouterState::new(crate::router::RouterStateConfig {
        store,
        scheduler,
        config: NodeConfig::default(),
        provider,
        tool_config: ToolConfig::default(),
        catalog: Arc::new(ToolCatalog::new()),
        domain_api: None,
        automaton_controller: None,
        automaton_bridge: None,
        memory_manager: None,
        skill_manager: None,
        router_url: None,
    })
}

fn create_test_store() -> Arc<dyn Store> {
    let dir = tempfile::tempdir().unwrap();
    Arc::new(RocksStore::open(dir.path(), false).unwrap())
}

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
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
    assert!(json["version"].is_string());
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

    let req = Request::builder()
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

    let req = Request::builder()
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

    let req = Request::builder()
        .method("POST")
        .uri("/tx")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
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

    let req = Request::builder()
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

    let req = Request::builder()
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

    let req = Request::builder()
        .method("POST")
        .uri("/tx")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn test_get_head_new_agent() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let agent_id = AgentId::generate();
    let req = Request::builder()
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

    let req = Request::builder()
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
    let req = Request::builder()
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
    let req = Request::builder()
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

    let req = Request::builder()
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

        let req = Request::builder()
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

    let req = Request::builder()
        .uri("/nonexistent")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ============================================================================
// Helper: RouterState with real memory + skill managers
// ============================================================================

fn test_router_state_with_managers() -> RouterState {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.keep();
    let rocks = RocksStore::open(&dir, false).unwrap();
    let db = rocks.db_handle().clone();
    let store: Arc<dyn Store> = Arc::new(rocks);

    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("mock"));
    let scheduler = Arc::new(Scheduler::new(
        store.clone(),
        provider.clone(),
        vec![],
        vec![],
        std::path::PathBuf::from("/tmp/workspaces"),
        None,
    ));

    let memory_kernel = Arc::new(
        Kernel::new(
            store.clone(),
            provider.clone(),
            ExecutorRouter::new(),
            KernelConfig::default(),
            AgentId::generate(),
        )
        .unwrap(),
    );
    let memory_gateway = Arc::new(KernelModelGateway::new(memory_kernel));
    let memory_manager = Arc::new(MemoryManager::new(
        db.clone(),
        memory_gateway,
        RefinerConfig::default(),
        WriteConfig::default(),
        RetrievalConfig::default(),
        ConsolidationConfig::default(),
        ProcedureConfig::default(),
    ));

    let skill_store = Arc::new(SkillInstallStore::new(db));
    let loader = SkillLoader::with_defaults(None, None);
    let skill_manager = Arc::new(std::sync::RwLock::new(SkillManager::with_install_store(
        loader,
        skill_store,
    )));

    RouterState::new(crate::router::RouterStateConfig {
        store,
        scheduler,
        config: NodeConfig::default(),
        provider,
        tool_config: ToolConfig::default(),
        catalog: Arc::new(ToolCatalog::new()),
        domain_api: None,
        automaton_controller: None,
        automaton_bridge: None,
        memory_manager: Some(memory_manager),
        skill_manager: Some(skill_manager),
        router_url: None,
    })
}

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
    let req = Request::builder()
        .method("POST")
        .uri(format!("/memory/{}/facts", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
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
    let req = Request::builder()
        .method("POST")
        .uri(format!("/memory/{}/facts", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
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
    let req = Request::builder()
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

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/memory/{}/facts/{}", agent_id.to_hex(), fact_id))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let req = Request::builder()
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
    let req = Request::builder()
        .method("POST")
        .uri(format!("/memory/{}/events", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
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
    let req = Request::builder()
        .method("POST")
        .uri(format!("/memory/{}/events", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let _ = app.clone().oneshot(req).await.unwrap();

    let bulk_delete_body = serde_json::json!({
        "before": chrono::Utc::now().to_rfc3339()
    });
    let req = Request::builder()
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

    let req = Request::builder()
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
    let req = Request::builder()
        .method("POST")
        .uri(format!("/memory/{}/procedures", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
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

    let req = Request::builder()
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
    let req = Request::builder()
        .method("POST")
        .uri(format!("/memory/{}/facts", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let _ = app.clone().oneshot(req).await.unwrap();

    let req = Request::builder()
        .method("POST")
        .uri(format!("/memory/{}/wipe", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let req = Request::builder()
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
    let req = Request::builder()
        .method("POST")
        .uri(format!("/memory/{}/facts", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let _ = app.clone().oneshot(req).await.unwrap();

    let req = Request::builder()
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

    let req = Request::builder()
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

    let req = Request::builder()
        .uri(format!("/memory/{}/facts", agent_id.to_hex()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ============================================================================
// Skills
// ============================================================================

#[tokio::test]
async fn test_skills_list() {
    let state = test_router_state_with_managers();
    let app = create_router(state);

    let req = Request::builder()
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

    let req = Request::builder()
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
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/agents/{}/skills", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = Request::builder()
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
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/agents/{}/skills", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = Request::builder()
        .method("DELETE")
        .uri(format!(
            "/api/agents/{}/skills/removable",
            agent_id.to_hex()
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let req = Request::builder()
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
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/harness/agents/{}/skills", agent_id.to_hex()))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = Request::builder()
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

    let req = Request::builder()
        .method("DELETE")
        .uri(format!(
            "/api/harness/agents/{}/skills/legacy-skill",
            agent_id.to_hex()
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let req = Request::builder()
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

    let req = Request::builder()
        .uri("/api/skills")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}
