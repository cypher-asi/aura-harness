//! HTTP and WebSocket router for the node API.

use crate::config::NodeConfig;
use crate::scheduler::Scheduler;
use crate::session::{handle_ws_connection, WsContext};
use aura_core::{AgentId, Transaction, TransactionType};
use aura_reasoner::ModelProvider;
use aura_store::Store;
use aura_tools::ToolConfig;
use axum::{
    extract::{ws::WebSocketUpgrade, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing::{error, info, instrument};

/// Shared state for the router.
pub struct RouterState {
    pub store: Arc<dyn Store>,
    pub scheduler: Arc<Scheduler>,
    pub config: NodeConfig,
    /// Model provider for WebSocket sessions (type-erased).
    pub provider: Arc<dyn ModelProvider + Send + Sync>,
    /// Tool configuration for WebSocket sessions.
    pub tool_config: ToolConfig,
}

impl Clone for RouterState {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            scheduler: self.scheduler.clone(),
            config: self.config.clone(),
            provider: self.provider.clone(),
            tool_config: self.tool_config.clone(),
        }
    }
}

/// Create the router.
pub fn create_router(state: RouterState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/tx", post(submit_tx_handler))
        .route("/agents/:agent_id/head", get(get_head_handler))
        .route("/agents/:agent_id/record", get(scan_record_handler))
        .route("/stream", get(ws_upgrade_handler))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

// === Health ===

/// Return a simple health-check response with version info.
async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

// === Submit Transaction ===

#[derive(Debug, Deserialize)]
struct SubmitTxRequest {
    agent_id: String,
    kind: String,
    payload: String,
}

#[derive(Debug, Serialize)]
struct SubmitTxResponse {
    accepted: bool,
    tx_id: String,
}

/// Accept a transaction submission, enqueue it, and schedule the agent for processing.
#[instrument(skip(state, request))]
async fn submit_tx_handler(
    State(state): State<RouterState>,
    Json(request): Json<SubmitTxRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let agent_id = AgentId::from_hex(&request.agent_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid agent_id: {e}")))?;

    let tx_type = match request.kind.as_str() {
        "user_prompt" => TransactionType::UserPrompt,
        "agent_msg" => TransactionType::AgentMsg,
        "trigger" => TransactionType::Trigger,
        "action_result" => TransactionType::ActionResult,
        "system" => TransactionType::System,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Invalid kind: {}", request.kind),
            ))
        }
    };

    use base64::Engine;
    let payload = base64::engine::general_purpose::STANDARD
        .decode(&request.payload)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Invalid payload encoding: {e}"),
            )
        })?;

    let tx = Transaction::new_chained(agent_id, tx_type, Bytes::from(payload), None);

    state.store.enqueue_tx(&tx).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Storage error: {e}"),
        )
    })?;

    info!(hash = %tx.hash, agent_id = %agent_id, "Transaction enqueued");

    let scheduler = state.scheduler.clone();
    tokio::spawn(async move {
        if let Err(e) = scheduler.schedule_agent(agent_id).await {
            error!(error = %e, "Failed to process agent");
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(SubmitTxResponse {
            accepted: true,
            tx_id: tx.hash.to_hex(),
        }),
    ))
}

// === Get Head ===

#[derive(Debug, Serialize)]
struct GetHeadResponse {
    agent_id: String,
    head_seq: u64,
}

/// Return the current head sequence number for a given agent.
#[instrument(skip(state))]
async fn get_head_handler(
    State(state): State<RouterState>,
    Path(agent_id_hex): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let agent_id = AgentId::from_hex(&agent_id_hex)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid agent_id: {e}")))?;

    let head_seq = state.store.get_head_seq(agent_id).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Storage error: {e}"),
        )
    })?;

    Ok(Json(GetHeadResponse {
        agent_id: agent_id_hex,
        head_seq,
    }))
}

// === Scan Record ===

#[derive(Debug, Deserialize)]
struct ScanRecordQuery {
    #[serde(default = "default_from_seq")]
    from_seq: u64,
    #[serde(default = "default_limit")]
    limit: usize,
}

const fn default_from_seq() -> u64 {
    1
}

const fn default_limit() -> usize {
    100
}

/// Scan an agent's record from a given sequence number, returning up to `limit` entries.
#[instrument(skip(state))]
async fn scan_record_handler(
    State(state): State<RouterState>,
    Path(agent_id_hex): Path<String>,
    Query(query): Query<ScanRecordQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let agent_id = AgentId::from_hex(&agent_id_hex)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid agent_id: {e}")))?;

    let limit = query.limit.min(1000);

    let entries = state
        .store
        .scan_record(agent_id, query.from_seq, limit)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Storage error: {e}"),
            )
        })?;

    Ok(Json(entries))
}

// === WebSocket ===

/// Upgrade an HTTP connection to a WebSocket for interactive agent sessions.
async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<RouterState>,
) -> impl IntoResponse {
    let auth_token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(String::from);

    let ctx = WsContext {
        workspace_base: state.config.workspaces_path(),
        provider: state.provider.clone(),
        tool_config: state.tool_config.clone(),
        auth_token,
    };
    ws.on_upgrade(move |socket| handle_ws_connection(socket, ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core::AgentId;
    use aura_reasoner::MockProvider;
    use aura_store::RocksStore;
    use axum::body::Body;
    use axum::http::Request;
    use tower_dev::util::ServiceExt;

    fn test_router_state(store: Arc<dyn Store>) -> RouterState {
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("mock"));
        let scheduler = Arc::new(Scheduler::new(
            store.clone(),
            provider.clone(),
            vec![],
            vec![],
            std::path::PathBuf::from("/tmp/workspaces"),
        ));
        RouterState {
            store,
            scheduler,
            config: NodeConfig::default(),
            provider,
            tool_config: ToolConfig::default(),
        }
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
}
