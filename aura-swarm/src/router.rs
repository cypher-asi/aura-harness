//! HTTP and WebSocket router for the swarm API.

use crate::config::SwarmConfig;
use crate::scheduler::Scheduler;
use crate::session::{handle_ws_connection, WsContext};
use aura_core::{AgentId, Transaction, TransactionType};
use aura_reasoner::Reasoner;
use aura_store::Store;
use axum::{
    extract::{
        ws::WebSocketUpgrade,
        Path, Query, State,
    },
    http::StatusCode,
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
pub struct RouterState<S, R>
where
    S: Store + 'static,
    R: Reasoner + 'static,
{
    pub store: Arc<S>,
    pub scheduler: Arc<Scheduler<S, R>>,
    pub config: SwarmConfig,
}

impl<S, R> Clone for RouterState<S, R>
where
    S: Store + 'static,
    R: Reasoner + 'static,
{
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            scheduler: self.scheduler.clone(),
            config: self.config.clone(),
        }
    }
}

/// Create the router.
pub fn create_router<S, R>(state: RouterState<S, R>) -> Router
where
    S: Store + 'static,
    R: Reasoner + 'static,
{
    Router::new()
        .route("/health", get(health_handler))
        .route("/tx", post(submit_tx_handler::<S, R>))
        .route("/agents/:agent_id/head", get(get_head_handler::<S, R>))
        .route("/agents/:agent_id/record", get(scan_record_handler::<S, R>))
        .route("/stream", get(ws_upgrade_handler::<S, R>))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

// === Health ===

async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

// === Submit Transaction ===

#[derive(Debug, Deserialize)]
pub struct SubmitTxRequest {
    pub agent_id: String,
    pub kind: String,
    pub payload: String, // base64 encoded
}

#[derive(Debug, Serialize)]
pub struct SubmitTxResponse {
    pub accepted: bool,
    pub tx_id: String,
}

#[instrument(skip(state, request))]
async fn submit_tx_handler<S, R>(
    State(state): State<RouterState<S, R>>,
    Json(request): Json<SubmitTxRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)>
where
    S: Store + 'static,
    R: Reasoner + 'static,
{
    // Parse agent ID
    let agent_id = AgentId::from_hex(&request.agent_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid agent_id: {e}")))?;

    // Parse tx_type
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

    // Decode payload
    use base64::Engine;
    let payload = base64::engine::general_purpose::STANDARD
        .decode(&request.payload)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Invalid payload encoding: {e}"),
            )
        })?;

    // Create transaction (chained with no previous hash for now - API doesn't support chaining yet)
    let tx = Transaction::new_chained(agent_id, tx_type, Bytes::from(payload), None);

    // Enqueue transaction
    state.store.enqueue_tx(&tx).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Storage error: {e}"),
        )
    })?;

    info!(hash = %tx.hash, agent_id = %agent_id, "Transaction enqueued");

    // Trigger processing (fire and forget)
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
pub struct GetHeadResponse {
    pub agent_id: String,
    pub head_seq: u64,
}

#[instrument(skip(state))]
async fn get_head_handler<S, R>(
    State(state): State<RouterState<S, R>>,
    Path(agent_id_hex): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)>
where
    S: Store + 'static,
    R: Reasoner + 'static,
{
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
pub struct ScanRecordQuery {
    #[serde(default = "default_from_seq")]
    pub from_seq: u64,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

const fn default_from_seq() -> u64 {
    1
}

const fn default_limit() -> usize {
    100
}

#[instrument(skip(state))]
async fn scan_record_handler<S, R>(
    State(state): State<RouterState<S, R>>,
    Path(agent_id_hex): Path<String>,
    Query(query): Query<ScanRecordQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)>
where
    S: Store + 'static,
    R: Reasoner + 'static,
{
    let agent_id = AgentId::from_hex(&agent_id_hex)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid agent_id: {e}")))?;

    let limit = query.limit.min(1000); // Cap at 1000

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

async fn ws_upgrade_handler<S, R>(
    ws: WebSocketUpgrade,
    State(state): State<RouterState<S, R>>,
) -> impl IntoResponse
where
    S: Store + 'static,
    R: Reasoner + 'static,
{
    let ctx = WsContext {
        workspace_base: state.config.workspaces_path(),
    };
    ws.on_upgrade(move |socket| handle_ws_connection(socket, ctx))
}
