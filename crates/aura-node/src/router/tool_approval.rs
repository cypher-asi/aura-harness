//! `/tool-approval` — `POST`/`DELETE` endpoints for the Phase 6 single-use
//! tool approval registry.
//!
//! Flow:
//! 1. The kernel denies a `run_command` (or any `RequireApproval`) tool
//!    proposal and emits a
//!    [`aura_kernel::ToolDecision::NeedsApproval { args_hash, .. }`].
//! 2. An authenticated operator hex-encodes that `args_hash` and
//!    `POST /tool-approval` with `{agent_id, tool, args_hash_hex}`.
//! 3. The next kernel tool proposal whose canonical args hash matches
//!    consumes the entry and runs as if the tool were `AlwaysAllow`.
//!
//! `DELETE /tool-approval` lets the operator revoke a grant before it
//! is consumed.

use super::RouterState;
use aura_core::AgentId;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use tracing::{info, instrument};

/// Body for both `POST` and `DELETE /tool-approval`.
#[derive(Debug, Deserialize)]
pub(super) struct ToolApprovalRequest {
    pub agent_id: String,
    pub tool: String,
    /// Blake3 hash of the canonical JSON args, hex-encoded.
    pub args_hash_hex: String,
}

impl ToolApprovalRequest {
    fn decode(&self) -> Result<(AgentId, &str, [u8; 32]), (StatusCode, String)> {
        let agent_id = if let Ok(uuid) = uuid::Uuid::parse_str(&self.agent_id) {
            AgentId::from_uuid(uuid)
        } else {
            AgentId::from_hex(&self.agent_id)
                .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid agent_id: {e}")))?
        };
        let bytes = hex::decode(&self.args_hash_hex).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid args_hash_hex: {e}"),
            )
        })?;
        let hash: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                format!(
                    "args_hash_hex must decode to 32 bytes, got {}",
                    bytes.len()
                ),
            )
        })?;
        Ok((agent_id, self.tool.as_str(), hash))
    }
}

/// `POST /tool-approval` — register a single-use approval.
#[instrument(skip(state, request), fields(tool = %request.tool))]
pub(super) async fn grant_tool_approval_handler(
    State(state): State<RouterState>,
    Json(request): Json<ToolApprovalRequest>,
) -> Result<Response, (StatusCode, String)> {
    let (agent_id, tool, args_hash) = request.decode()?;
    state
        .scheduler
        .approval_registry()
        .grant(agent_id, tool, args_hash);
    info!(
        agent_id = %agent_id,
        tool,
        args_hash_hex = %request.args_hash_hex,
        "tool approval granted"
    );
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "status": "granted",
            "agent_id": request.agent_id,
            "tool": request.tool,
            "args_hash_hex": request.args_hash_hex,
        })),
    )
        .into_response())
}

/// `DELETE /tool-approval` — revoke a pending approval.
#[instrument(skip(state, request), fields(tool = %request.tool))]
pub(super) async fn revoke_tool_approval_handler(
    State(state): State<RouterState>,
    Json(request): Json<ToolApprovalRequest>,
) -> Result<Response, (StatusCode, String)> {
    let (agent_id, tool, args_hash) = request.decode()?;
    let removed = state
        .scheduler
        .approval_registry()
        .revoke(agent_id, tool, args_hash);
    info!(
        agent_id = %agent_id,
        tool,
        args_hash_hex = %request.args_hash_hex,
        removed,
        "tool approval revoke request"
    );
    if removed {
        Ok((
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "revoked",
                "agent_id": request.agent_id,
                "tool": request.tool,
                "args_hash_hex": request.args_hash_hex,
            })),
        )
            .into_response())
    } else {
        Err((
            StatusCode::NOT_FOUND,
            "no pending approval for the given (agent_id, tool, args_hash)".to_string(),
        ))
    }
}
