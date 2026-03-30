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
