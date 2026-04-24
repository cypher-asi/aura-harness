use super::errors::ApiError;
use super::ids::parse_agent_id;
use super::*;

#[derive(Debug, Deserialize)]
pub(super) struct SubmitTxRequest {
    agent_id: String,
    kind: String,
    payload: String,
}

#[derive(Debug, Serialize)]
pub(super) struct SubmitTxResponse {
    accepted: bool,
    tx_id: String,
}

/// Accept a transaction submission, enqueue it, and schedule the agent for processing.
#[instrument(skip(state, request))]
pub(super) async fn submit_tx_handler(
    State(state): State<RouterState>,
    Json(request): Json<SubmitTxRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let agent_id = parse_agent_id(&request.agent_id).map_err(ApiError::into_string_tuple)?;

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

    // Phase 5: reject mid-session `AgentPermissions` mutation.
    //
    // Per-session permissions are baked in at `SessionInit` time (see
    // `Session::apply_init`) and applied to the kernel `PolicyConfig`
    // during session bootstrap. Any attempt to ship a new permission
    // bundle via `/tx` (regardless of transaction kind) is refused so a
    // running session cannot escalate its own capabilities.
    if matches!(tx_type, TransactionType::System) && carries_agent_permissions_mutation(&payload) {
        return Err((
            StatusCode::FORBIDDEN,
            "permissions: AgentPermissions are frozen per session; send a new SessionInit instead"
                .to_string(),
        ));
    }

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
    let obj = match value {
        serde_json::Value::Object(obj) => obj,
        _ => return false,
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
pub(super) struct TxStatusResponse {
    tx_id: String,
    status: String,
}

/// Check the processing status of a previously submitted transaction.
#[instrument(skip(state))]
pub(super) async fn tx_status_handler(
    State(state): State<RouterState>,
    Path((agent_id_hex, tx_id_hex)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let agent_id = parse_agent_id(&agent_id_hex).map_err(ApiError::into_string_tuple)?;
    let tx_hash = Hash::from_hex(&tx_id_hex)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid tx_id: {e}")))?;

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

    if entries.iter().any(|e| e.tx.hash == tx_hash) {
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

    let status = if state.store.has_pending_tx(agent_id).unwrap_or(false) {
        "pending"
    } else {
        "unknown"
    };

    Ok(Json(TxStatusResponse {
        tx_id: tx_id_hex,
        status: status.to_string(),
    }))
}

// === Get Head ===

#[derive(Debug, Serialize)]
pub(super) struct GetHeadResponse {
    agent_id: String,
    head_seq: u64,
}

/// Return the current head sequence number for a given agent.
#[instrument(skip(state))]
pub(super) async fn get_head_handler(
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
pub(super) struct ScanRecordQuery {
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
pub(super) async fn scan_record_handler(
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
