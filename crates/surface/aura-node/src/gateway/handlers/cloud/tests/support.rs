//! Shared fixtures for the cloud route tests: a claimed cloud node's
//! router state, requests to its routes, and record imports.

use crate::config::NodeConfig;
use crate::gateway::{create_router, RouterState, RouterStateConfig};
use aura_core_protocol::cloud::NodeClaim;
use aura_core_types::{AgentId, RecordEntry, Transaction};
use aura_exec_tools::{ToolCatalog, ToolConfig};
use aura_model::{MockProvider, ModelProvider};
use aura_store_db::{RocksStore, Store};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::util::ServiceExt;

pub(super) const NODE_TOKEN: &str = "node-token-for-tests-0123";

pub(super) fn cloud_state(dir: &std::path::Path) -> RouterState {
    cloud_state_in(dir, None)
}

pub(super) fn cloud_state_in(
    dir: &std::path::Path,
    workspace: Option<std::path::PathBuf>,
) -> RouterState {
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(dir.join("db"), false).expect("store"));
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("mock"));
    let scheduler = Arc::new(aura_engine::scheduler::Scheduler::new(
        store.clone(),
        provider.clone(),
        vec![],
        vec![],
        dir.join("ws"),
        None,
    ));
    RouterState::new(RouterStateConfig {
        store,
        scheduler,
        config: NodeConfig {
            cloud_node: true,
            data_dir: dir.to_path_buf(),
            workspace,
            ..NodeConfig::default()
        },
        provider,
        tool_config: ToolConfig::default(),
        catalog: Arc::new(ToolCatalog::new()),
        domain_api: None,
        automaton_controller: None,
        automaton_bridge: None,
        memory_manager: None,
        skill_manager: None,
        secrets_vault: None,
        process_store: None,
        trigger_registrar: None,
        router_url: None,
    })
}

pub(super) fn claim(state: &RouterState) {
    state.cloud.apply_claim(
        NodeClaim {
            node_id: "aura-re36-3f9c".into(),
            node_token: NODE_TOKEN.into(),
            mesh_token: "mesh-token-for-tests-0123".into(),
            seal: Vec::new(),
            owner: None,
            plans: Vec::new(),
            git: None,
            public_url: None,
        },
        8080,
    );
}

pub(super) async fn call(
    state: &RouterState,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let body = match body {
        Some(json) => {
            request = request.header("content-type", "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = create_router(state.clone())
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

pub(super) async fn import(state: &RouterState, agent: AgentId, prompts: u64) {
    let entries: Vec<RecordEntry> = (1..=prompts)
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
    let (status, body) = call(
        state,
        "POST",
        "/v1/sync/import",
        Some(NODE_TOKEN),
        Some(import),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
