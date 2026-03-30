use crate::record_loader::extract_tool_info;
use aura_core::{AgentId, EffectStatus, RecordEntry, Transaction, TransactionType};
use aura_terminal::{
    events::{RecordStatus, RecordSummary},
    UiCommand,
};
use tokio::sync::mpsc;

/// Send a record summary to the UI (matching the stored format).
pub(super) async fn send_record_to_ui(
    commands: &mpsc::Sender<UiCommand>,
    seq: u64,
    tx: &Transaction,
    entry: &RecordEntry,
) {
    let (tx_kind, sender) = match tx.tx_type {
        TransactionType::UserPrompt => ("Prompt".to_string(), "USER".to_string()),
        TransactionType::ActionResult => ("Action".to_string(), "SYSTEM".to_string()),
        TransactionType::System => ("System".to_string(), "SYSTEM".to_string()),
        TransactionType::AgentMsg => ("Response".to_string(), "AURA".to_string()),
        TransactionType::Trigger => ("Trigger".to_string(), "SYSTEM".to_string()),
        TransactionType::SessionStart => ("Session".to_string(), "SYSTEM".to_string()),
        TransactionType::ToolProposal => ("Propose".to_string(), "LLM".to_string()),
        TransactionType::ToolExecution => ("Execute".to_string(), "KERNEL".to_string()),
        TransactionType::ProcessComplete => ("Complete".to_string(), "SYSTEM".to_string()),
    };

    let message = String::from_utf8_lossy(&tx.payload).to_string();
    let message = if message.len() > 200 {
        format!("{}...", &message[..197])
    } else {
        message
    };

    let effect_count = entry.effects.len();
    let ok_count = entry
        .effects
        .iter()
        .filter(|e| matches!(e.status, EffectStatus::Committed))
        .count();
    let pending_count = entry
        .effects
        .iter()
        .filter(|e| matches!(e.status, EffectStatus::Pending))
        .count();
    let err_count = effect_count - ok_count - pending_count;

    let effect_status = if effect_count == 0 {
        "-".to_string()
    } else if err_count == 0 {
        format!("{ok_count} ok")
    } else {
        format!("{ok_count} ok, {err_count} err")
    };

    let status = if err_count > 0 {
        RecordStatus::Error
    } else if pending_count > 0 {
        RecordStatus::Pending
    } else {
        RecordStatus::Ok
    };

    let error_details: String = entry
        .effects
        .iter()
        .filter(|e| matches!(e.status, EffectStatus::Failed))
        .filter_map(|e| String::from_utf8(e.payload.to_vec()).ok())
        .collect::<Vec<_>>()
        .join("; ");

    let info = extract_tool_info(tx);

    let full_hash = hex::encode(entry.context_hash);
    let hash_suffix = full_hash[full_hash.len() - 4..].to_string();

    let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();

    let record_summary = RecordSummary {
        seq,
        timestamp,
        full_hash,
        hash_suffix,
        tx_kind,
        sender,
        message,
        action_count: entry.actions.len(),
        effect_status,
        status,
        info,
        error_details,
        tx_id: hex::encode(tx.hash.as_bytes()),
        agent_id: hex::encode(tx.agent_id.as_bytes()),
        ts_ms: tx.ts_ms,
    };

    let _ = commands.send(UiCommand::NewRecord(record_summary)).await;
}

/// Compute a context hash for a record entry.
pub(super) fn compute_context_hash(seq: u64, tx: &Transaction) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&seq.to_be_bytes());
    hasher.update(tx.hash.as_bytes());
    hasher.update(&tx.ts_ms.to_be_bytes());
    hasher.update(&tx.payload);
    *hasher.finalize().as_bytes()
}

/// Create a response transaction for the assistant's message.
pub(super) fn create_response_transaction(agent_id: AgentId, response_text: &str) -> Transaction {
    Transaction::new_chained(
        agent_id,
        TransactionType::AgentMsg,
        response_text.as_bytes().to_vec(),
        None,
    )
}
