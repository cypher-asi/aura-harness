use super::super::errors::ApiError;
use super::util::parse_agent_id;
use crate::gateway::RouterState;
use aura_core_types::{Hash, Transaction, TransactionType};
use aura_core_types::{PolicyDecision, RecordEntry};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tracing::{error, info, instrument};

#[derive(Debug, Deserialize)]
pub(in crate::gateway) struct SubmitTxRequest {
    agent_id: String,
    kind: String,
    payload: String,
}

#[derive(Debug, Serialize)]
pub(in crate::gateway) struct SubmitTxResponse {
    accepted: bool,
    tx_id: String,
}

/// Accept a transaction submission, enqueue it, and schedule the agent for processing.
#[instrument(skip(state, request))]
pub(in crate::gateway) async fn submit_tx_handler(
    State(state): State<RouterState>,
    Json(request): Json<SubmitTxRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let agent_id = parse_agent_id(&request.agent_id).map_err(ApiError::into_string_tuple)?;
    if state.cloud.is_sealed(&agent_id) {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "agent {agent_id} was handed back to its hub; this node no longer writes for it"
            ),
        ));
    }

    let tx_type = parse_tx_kind(&request.kind)?;
    let payload = decode_tx_payload(tx_type, &request.payload)?;
    let tx = Transaction::new_chained(agent_id, tx_type, Bytes::from(payload), None);

    state.store.enqueue_tx(&tx).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Storage error: {e}"),
        )
    })?;

    info!(hash = %tx.hash, agent_id = %agent_id, "Transaction enqueued");

    let scheduler = state.scheduler.clone();
    let failed_txs = state.failed_txs.clone();
    let tx_id_hex = tx.hash.to_hex();
    tokio::spawn(async move {
        if let Err(e) = scheduler.schedule_agent(agent_id).await {
            error!(error = %e, "Failed to process agent");
            failed_txs.insert(tx_id_hex, e.to_string());
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

fn parse_tx_kind(kind: &str) -> Result<TransactionType, (StatusCode, String)> {
    match kind {
        "user_prompt" => Ok(TransactionType::UserPrompt),
        "agent_msg" => Ok(TransactionType::AgentMsg),
        "trigger" => Ok(TransactionType::Trigger),
        "action_result" => Ok(TransactionType::ActionResult),
        "system" => Ok(TransactionType::System),
        _ => Err((StatusCode::BAD_REQUEST, format!("Invalid kind: {kind}"))),
    }
}

/// Decode the base64 payload, refuse permission mutations, and wrap
/// prompt text as a prompt payload.
fn decode_tx_payload(
    tx_type: TransactionType,
    encoded: &str,
) -> Result<Vec<u8>, (StatusCode, String)> {
    use base64::Engine;
    let payload = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Invalid payload encoding: {e}"),
            )
        })?;

    // Phase 5: reject mid-session `AgentPermissions` mutation.
    //
    // Per-run permissions are baked in from the `RuntimeRequest` and
    // applied to the kernel `PolicyConfig` during session bootstrap. Any
    // attempt to ship a new permission bundle via `/tx` (regardless of
    // transaction kind) is refused so a running session cannot escalate its
    // own capabilities.
    if matches!(tx_type, TransactionType::System) && carries_agent_permissions_mutation(&payload) {
        return Err((
            StatusCode::FORBIDDEN,
            "permissions: AgentPermissions are frozen per run; start a new run instead".to_string(),
        ));
    }

    // A submitted prompt is its text; the record keeps it as a prompt
    // payload like every other prompt.
    if tx_type == TransactionType::UserPrompt {
        let text = String::from_utf8(payload).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Prompt payload is not UTF-8 text: {e}"),
            )
        })?;
        Ok(aura_core_types::PromptPayload::new(text).to_bytes())
    } else {
        Ok(payload)
    }
}

/// Phase 5: detect whether a `System`-kind transaction payload is
/// attempting to mutate `AgentPermissions`. We accept two encodings:
///
/// - An explicit `{"kind": "agent_permissions", ...}` marker.
/// - Any JSON object that contains a top-level `agent_permissions` key.
///
/// Non-JSON payloads and JSON payloads without either marker pass
/// through untouched (these are the normal System-tx shapes used by the
/// kernel for identity / delegate bookkeeping, which are written by the
/// kernel itself rather than via `/tx`).
fn carries_agent_permissions_mutation(payload: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload) else {
        return false;
    };
    let serde_json::Value::Object(obj) = value else {
        return false;
    };
    if let Some(kind) = obj.get("kind").and_then(|v| v.as_str()) {
        if kind == "agent_permissions" || kind == "set_agent_permissions" {
            return true;
        }
    }
    obj.contains_key("agent_permissions")
}

// === Tx Status ===

#[derive(Debug, Serialize)]
pub(in crate::gateway) struct TxStatusResponse {
    tx_id: String,
    status: String,
}

/// Check the processing status of a previously submitted transaction.
#[instrument(skip(state))]
pub(in crate::gateway) async fn tx_status_handler(
    State(state): State<RouterState>,
    Path((agent_id_hex, tx_id_hex)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let agent_id = parse_agent_id(&agent_id_hex).map_err(ApiError::into_string_tuple)?;
    let tx_hash = Hash::from_hex(&tx_id_hex)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid tx_id: {e}")))?;

    if recent_record_contains(&state, agent_id, tx_hash)? {
        return Ok(Json(TxStatusResponse {
            tx_id: tx_id_hex,
            status: "processed".to_string(),
        }));
    }

    if let Some(err) = state.failed_txs.get(&tx_id_hex) {
        return Ok(Json(TxStatusResponse {
            tx_id: tx_id_hex,
            status: format!("failed: {}", err.value()),
        }));
    }

    let status = pending_tx_status(&state, agent_id, &agent_id_hex, &tx_id_hex)?;
    Ok(Json(TxStatusResponse {
        tx_id: tx_id_hex,
        status: status.to_string(),
    }))
}

/// Whether the last 100 record entries include `tx_hash`.
fn recent_record_contains(
    state: &RouterState,
    agent_id: aura_core_types::AgentId,
    tx_hash: Hash,
) -> Result<bool, (StatusCode, String)> {
    let head_seq = state.store.get_head_seq(agent_id).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Storage error: {e}"),
        )
    })?;

    let from_seq = head_seq.saturating_sub(100).max(1);
    let entries = state
        .store
        .scan_record(agent_id, from_seq, 100)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Storage error: {e}"),
            )
        })?;
    Ok(entries.iter().any(|e| e.tx.hash == tx_hash))
}

fn pending_tx_status(
    state: &RouterState,
    agent_id: aura_core_types::AgentId,
    agent_id_hex: &str,
    tx_id_hex: &str,
) -> Result<&'static str, (StatusCode, String)> {
    // Phase 5 (error-handling polish): we used to fall back to
    // `"unknown"` on `has_pending_tx` errors via `.unwrap_or(false)`,
    // which silently mapped storage failures to a generic status the
    // WS client could not distinguish from "tx never submitted".
    // Surface the error to the caller (and trace it) so a misbehaving
    // store does not get hidden behind a benign-looking status code.
    match state.store.has_pending_tx(agent_id) {
        Ok(true) => Ok("pending"),
        Ok(false) => Ok("unknown"),
        Err(e) => {
            tracing::error!(
                error = %e,
                agent_id = %agent_id_hex,
                tx_id = %tx_id_hex,
                "tx_status: store.has_pending_tx failed"
            );
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Storage error: {e}"),
            ))
        }
    }
}

// === Get Head ===

#[derive(Debug, Serialize)]
pub(in crate::gateway) struct GetHeadResponse {
    agent_id: String,
    head_seq: u64,
}

/// Return the current head sequence number for a given agent.
#[instrument(skip(state))]
pub(in crate::gateway) async fn get_head_handler(
    State(state): State<RouterState>,
    Path(agent_id_hex): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let agent_id = parse_agent_id(&agent_id_hex).map_err(ApiError::into_string_tuple)?;

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
pub(in crate::gateway) struct ScanRecordQuery {
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
pub(in crate::gateway) async fn scan_record_handler(
    State(state): State<RouterState>,
    Path(agent_id_hex): Path<String>,
    Query(query): Query<ScanRecordQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let agent_id = parse_agent_id(&agent_id_hex).map_err(ApiError::into_string_tuple)?;

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

// === Tool Call Audit ===

/// How far back from head the audit endpoint searches for a decision.
const AUDIT_DECISION_SCAN_LIMIT: u64 = 10_000;
/// How far past the decision the audit endpoint searches for its execution
/// (batched calls record every decision before any execution).
const AUDIT_EXECUTION_SCAN_LIMIT: usize = 1_000;
const AUDIT_SCAN_PAGE: usize = 500;

#[derive(Debug, Serialize)]
pub(in crate::gateway) struct ToolCallAuditResponse {
    agent_id: String,
    tool_use_id: String,
    /// The `ToolProposal` entry the decision references.
    proposal: Option<RecordEntry>,
    /// The `PolicyDecision` entry.
    policy_decision: RecordEntry,
    /// The decoded decision payload (verdict, rule, trace, approval).
    decision: PolicyDecision,
    /// The `ToolExecution` entry; absent for denied or pending calls.
    execution: Option<RecordEntry>,
}

/// Return the proposal → policy decision → execution chain for one tool call.
#[instrument(skip(state))]
pub(in crate::gateway) async fn tool_call_audit_handler(
    State(state): State<RouterState>,
    Path((agent_id_hex, tool_use_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let agent_id = parse_agent_id(&agent_id_hex).map_err(ApiError::into_string_tuple)?;
    let Some((decision_entry, decision)) = find_policy_decision(&state, agent_id, &tool_use_id)?
    else {
        return Err((
            StatusCode::NOT_FOUND,
            format!("No policy decision recorded for tool_use_id '{tool_use_id}'"),
        ));
    };

    let proposal = match (decision_entry.tx.reference_tx_hash, decision_entry.seq) {
        (Some(proposal_hash), seq) if seq > 1 => state
            .store
            .get_record_entry(agent_id, seq - 1)
            .ok()
            .filter(|entry| entry.tx.hash == proposal_hash),
        _ => None,
    };

    let execution = state
        .store
        .scan_record(agent_id, decision_entry.seq + 1, AUDIT_EXECUTION_SCAN_LIMIT)
        .map_err(|e| audit_storage_err(&e))?
        .into_iter()
        .find(|entry| {
            entry.tx.tx_type == TransactionType::ToolExecution
                && entry.tx.reference_tx_hash == Some(decision_entry.tx.hash)
        });

    Ok(Json(ToolCallAuditResponse {
        agent_id: agent_id_hex,
        tool_use_id,
        proposal,
        policy_decision: decision_entry,
        decision,
        execution,
    }))
}

fn audit_storage_err(e: &aura_store_db::StoreError) -> (StatusCode, String) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("Storage error: {e}"),
    )
}

/// Page backwards from head looking for the `PolicyDecision` of `tool_use_id`.
fn find_policy_decision(
    state: &RouterState,
    agent_id: aura_core_types::AgentId,
    tool_use_id: &str,
) -> Result<Option<(RecordEntry, PolicyDecision)>, (StatusCode, String)> {
    let storage_err = |e: aura_store_db::StoreError| audit_storage_err(&e);
    let head_seq = state.store.get_head_seq(agent_id).map_err(storage_err)?;
    let floor = head_seq.saturating_sub(AUDIT_DECISION_SCAN_LIMIT).max(1);
    let mut cursor = head_seq;
    let mut found: Option<(RecordEntry, PolicyDecision)> = None;
    while cursor >= floor && found.is_none() {
        let page = state
            .store
            .scan_record_descending(agent_id, cursor, AUDIT_SCAN_PAGE)
            .map_err(storage_err)?;
        let Some(last) = page.last() else { break };
        let next_cursor = last.seq.saturating_sub(1);
        found = page.into_iter().find_map(|entry| {
            if entry.tx.tx_type != TransactionType::PolicyDecision {
                return None;
            }
            let decision: PolicyDecision = serde_json::from_slice(&entry.tx.payload).ok()?;
            (decision.tool_use_id == tool_use_id).then_some((entry, decision))
        });
        if next_cursor == 0 {
            break;
        }
        cursor = next_cursor;
    }
    Ok(found)
}
