//! Loading and displaying historical records from the store.

use crate::agent_registry::AgentRegistry;
use aura_core_types::{AgentId, EffectStatus, RecordEntry, Transaction, TransactionType};
use aura_store_db::{ReadStore, StoreError};
use aura_terminal::{
    events::{AgentSummary, MessageData, MessageRole, RecordStatus, RecordSummary},
    UiCommand,
};
use tokio::sync::mpsc;
use tracing::warn;

/// Number of entries shown in the Record panel.
const RECORD_PANEL_LIMIT: usize = 100;

/// Number of trailing entries scanned when rebuilding an agent's chat.
const HISTORY_SCAN_LIMIT: usize = 500;

/// Number of chat messages kept after rebuilding (matches the UI cap).
const HISTORY_MESSAGE_LIMIT: usize = 100;

/// Load the most recent record entries for an agent as UI summaries,
/// oldest first.
///
/// # Errors
/// Returns the store error if the head sequence or scan fails.
pub fn recent_record_summaries(
    store: &dyn ReadStore,
    agent_id: AgentId,
) -> Result<Vec<RecordSummary>, StoreError> {
    Ok(scan_tail(store, agent_id, RECORD_PANEL_LIMIT)?
        .iter()
        .map(record_summary)
        .collect())
}

/// Load existing records from the store and send to UI.
///
/// Uses `try_send`, so it is only suitable before the UI starts draining
/// the channel (startup); async callers should send summaries themselves.
pub fn load_existing_records(
    store: &dyn ReadStore,
    agent_id: AgentId,
    commands: &mpsc::Sender<UiCommand>,
) {
    match recent_record_summaries(store, agent_id) {
        Ok(summaries) => {
            for summary in summaries {
                let _ = commands.try_send(UiCommand::NewRecord(summary));
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to load records");
            let _ = commands.try_send(UiCommand::ShowWarning(format!(
                "Could not load record history: {e}"
            )));
        }
    }
}

/// Rebuild an agent's chat transcript from its record, oldest first.
///
/// # Errors
/// Returns the store error if the head sequence or scan fails.
pub fn conversation_history(
    store: &dyn ReadStore,
    agent_id: AgentId,
) -> Result<Vec<MessageData>, StoreError> {
    let history: Vec<MessageData> = scan_tail(store, agent_id, HISTORY_SCAN_LIMIT)?
        .iter()
        .filter_map(|entry| history_message(&entry.tx))
        .collect();
    let skip = history.len().saturating_sub(HISTORY_MESSAGE_LIMIT);
    Ok(history.into_iter().skip(skip).collect())
}

/// Build the agents-column rows for every registered agent.
#[must_use]
pub fn agent_summaries(
    registry: &AgentRegistry,
    store: &dyn ReadStore,
    active: AgentId,
) -> Vec<AgentSummary> {
    registry
        .agents()
        .iter()
        .map(|agent| {
            let agent_id = agent.agent_id();
            let record_count = store.get_head_seq(agent_id).unwrap_or(0);
            let last_active = if record_count == 0 {
                "-".to_string()
            } else {
                store
                    .get_record_entry(agent_id, record_count)
                    .map(|entry| format_local_time(entry.tx.ts_ms))
                    .unwrap_or_else(|_| "-".to_string())
            };
            AgentSummary {
                id: agent_id.to_hex(),
                name: agent.name.clone(),
                zns_id: agent.zns_id.clone(),
                is_active: agent_id == active,
                record_count,
                last_active,
            }
        })
        .collect()
}

fn scan_tail(
    store: &dyn ReadStore,
    agent_id: AgentId,
    limit: usize,
) -> Result<Vec<RecordEntry>, StoreError> {
    let head_seq = store.get_head_seq(agent_id)?;
    if head_seq == 0 {
        return Ok(Vec::new());
    }
    let window = u64::try_from(limit).unwrap_or(u64::MAX);
    let from_seq = head_seq.saturating_sub(window.saturating_sub(1)).max(1);
    store.scan_record(agent_id, from_seq, limit)
}

fn history_message(tx: &Transaction) -> Option<MessageData> {
    let (role, content) = match tx.tx_type {
        TransactionType::UserPrompt => (
            MessageRole::User,
            String::from_utf8_lossy(&tx.payload).into_owned(),
        ),
        TransactionType::AgentMsg => (
            MessageRole::Assistant,
            String::from_utf8_lossy(&tx.payload).into_owned(),
        ),
        TransactionType::ToolExecution => {
            let info = extract_tool_info(tx);
            if info.is_empty() {
                return None;
            }
            (MessageRole::System, format!("🔧 {info}"))
        }
        TransactionType::SessionStart => (MessageRole::System, "── new session ──".to_string()),
        _ => return None,
    };
    if content.trim().is_empty() {
        return None;
    }
    Some(MessageData {
        role,
        content,
        is_streaming: false,
        timestamp_ms: Some(tx.ts_ms),
    })
}

fn record_summary(entry: &RecordEntry) -> RecordSummary {
    let (tx_kind, sender) = tx_type_label(entry.tx.tx_type);

    let message = String::from_utf8_lossy(&entry.tx.payload).to_string();
    let message = if message.chars().count() > 200 {
        let truncated: String = message.chars().take(197).collect();
        format!("{truncated}...")
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

    let full_hash = entry.context_hash.as_hex();
    let hash_suffix = full_hash[full_hash.len() - 4..].to_string();

    RecordSummary {
        seq: entry.seq,
        timestamp: format_local_time(entry.tx.ts_ms),
        full_hash,
        hash_suffix,
        tx_kind: tx_kind.to_string(),
        sender: sender.to_string(),
        message,
        action_count: entry.actions.len(),
        effect_status,
        status,
        info: extract_tool_info(&entry.tx),
        error_details,
        tx_id: hex::encode(entry.tx.hash.as_bytes()),
        agent_id: hex::encode(entry.tx.agent_id.as_bytes()),
        ts_ms: entry.tx.ts_ms,
    }
}

fn format_local_time(ts_ms: u64) -> String {
    i64::try_from(ts_ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "??:??:??".to_string())
}

/// Map a transaction type to its display label and sender name.
pub fn tx_type_label(tx_type: TransactionType) -> (&'static str, &'static str) {
    match tx_type {
        TransactionType::UserPrompt => ("Prompt", "USER"),
        TransactionType::ActionResult => ("Action", "SYSTEM"),
        TransactionType::System => ("System", "SYSTEM"),
        TransactionType::AgentMsg => ("Response", "AURA"),
        TransactionType::Trigger => ("Trigger", "SYSTEM"),
        TransactionType::SessionStart => ("Session", "SYSTEM"),
        TransactionType::ToolProposal => ("Propose", "LLM"),
        TransactionType::ToolExecution => ("Execute", "KERNEL"),
        TransactionType::ProcessComplete => ("Complete", "SYSTEM"),
        TransactionType::Reasoning => ("Reasoning", "KERNEL"),
        TransactionType::SubagentSpawn => ("Spawn", "FLEET"),
    }
}

/// Extract tool name or other info from a transaction payload.
pub fn extract_tool_info(tx: &Transaction) -> String {
    match tx.tx_type {
        TransactionType::ToolProposal => {
            if let Ok(proposal) =
                serde_json::from_slice::<aura_core_types::ToolProposal>(&tx.payload)
            {
                if proposal.tool == "run_command" {
                    return extract_cmd_run_command(&proposal.args);
                }
                return proposal.tool;
            }
        }
        TransactionType::ToolExecution => {
            if let Ok(execution) =
                serde_json::from_slice::<aura_core_types::ToolExecution>(&tx.payload)
            {
                if execution.tool == "run_command" {
                    return extract_cmd_run_command(&execution.args);
                }
                return execution.tool;
            }
        }
        _ => {}
    }

    String::new()
}

/// Extract the command string from cmd_run tool arguments.
fn extract_cmd_run_command(args: &serde_json::Value) -> String {
    let program = args["program"].as_str().unwrap_or("");
    let cmd_args: Vec<&str> = args["args"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    if cmd_args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", cmd_args.join(" "))
    }
}

#[cfg(test)]
mod tests;
