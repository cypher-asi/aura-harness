//! Event processing loop for the terminal UI mode.

use crate::record_loader::extract_tool_info;
use aura_agent::{AgentLoop, KernelToolExecutor};
use aura_core::{AgentId, EffectStatus, RecordEntry, Transaction, TransactionType};
use aura_kernel::ProcessManager;
use aura_reasoner::{Message, ModelProvider, ToolDefinition};
use aura_store::{RocksStore, Store};
use aura_terminal::{
    events::{RecordStatus, RecordSummary},
    UiCommand, UiEvent,
};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Run the event processing loop.
///
/// Handles user messages from the UI and process completion events.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_event_loop(
    events: &mut mpsc::Receiver<UiEvent>,
    mut process_completions: mpsc::Receiver<Transaction>,
    commands: mpsc::Sender<UiCommand>,
    agent_loop: &AgentLoop,
    provider: &dyn ModelProvider,
    executor: &KernelToolExecutor,
    tools: &[ToolDefinition],
    store: Arc<RocksStore>,
    agent_id: AgentId,
    _process_manager: Arc<ProcessManager>,
) -> anyhow::Result<()> {
    let mut seq = store.get_head_seq(agent_id).unwrap_or(0) + 1;
    let mut messages: Vec<Message> = Vec::new();

    loop {
        tokio::select! {
            Some(completion_tx) = process_completions.recv() => {
                info!(
                    hash = %completion_tx.hash,
                    tx_type = ?completion_tx.tx_type,
                    "Processing async process completion"
                );

                if let Err(e) = store.enqueue_tx(&completion_tx) {
                    error!(error = %e, "Failed to enqueue completion transaction");
                    continue;
                }

                if let Ok(Some((inbox_seq, tx))) = store.dequeue_tx(agent_id) {
                    let context_hash = compute_context_hash(seq, &tx);
                    let entry = RecordEntry::builder(seq, tx.clone())
                        .context_hash(context_hash)
                        .build();

                    if let Err(e) = store.append_entry_atomic(agent_id, seq, &entry, inbox_seq) {
                        error!(error = %e, "Failed to persist completion record");
                    } else {
                        debug!(seq = seq, "Completion record persisted");
                        send_record_to_ui(&commands, seq, &tx, &entry).await;
                        seq += 1;

                        let _ = commands.send(UiCommand::SetStatus("Process completed".to_string())).await;
                    }
                }
            }

            Some(event) = events.recv() => {
                match event {
            UiEvent::UserMessage(text) => {
                info!(text = %text, seq = seq, "Processing user message");

                let _ = commands
                    .send(UiCommand::SetStatus("Thinking...".to_string()))
                    .await;

                let mut stale_count = 0;
                while let Ok(Some((stale_inbox_seq, stale_tx))) = store.dequeue_tx(agent_id) {
                    warn!(
                        stale_inbox_seq = stale_inbox_seq,
                        stale_tx_type = ?stale_tx.tx_type,
                        "Discarding stale inbox transaction"
                    );
                    let stale_entry = RecordEntry::builder(seq, stale_tx.clone())
                        .context_hash(compute_context_hash(seq, &stale_tx))
                        .build();
                    if let Err(e) = store.append_entry_atomic(agent_id, seq, &stale_entry, stale_inbox_seq) {
                        error!(error = %e, "Failed to clear stale transaction");
                        break;
                    }
                    seq += 1;
                    stale_count += 1;
                    if stale_count > 10 {
                        error!("Too many stale transactions, aborting drain");
                        break;
                    }
                }

                let tx = Transaction::user_prompt(agent_id, text.clone());
                if let Err(e) = store.enqueue_tx(&tx) {
                    error!(error = %e, "Failed to enqueue transaction");
                    let _ = commands
                        .send(UiCommand::ShowError(format!("Storage error: {e}")))
                        .await;
                    let _ = commands.send(UiCommand::Complete).await;
                    continue;
                }

                let (inbox_seq, dequeued_tx) = match store.dequeue_tx(agent_id) {
                    Ok(Some(item)) => item,
                    Ok(None) => {
                        error!("Transaction was enqueued but not found in inbox");
                        continue;
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to dequeue transaction");
                        continue;
                    }
                };

                if dequeued_tx.hash != tx.hash {
                    error!("Transaction mismatch after draining stale entries");
                }

                messages.push(Message::user(text));

                let process_result = agent_loop
                    .run(provider, executor, messages.clone(), tools.to_vec())
                    .await;

                match process_result {
                    Ok(result) => {
                        let prompt_context_hash = compute_context_hash(seq, &tx);
                        let prompt_entry = RecordEntry::builder(seq, tx.clone())
                            .context_hash(prompt_context_hash)
                            .build();

                        if let Err(e) =
                            store.append_entry_atomic(agent_id, seq, &prompt_entry, inbox_seq)
                        {
                            error!(error = %e, "Failed to persist prompt record");
                            let _ = commands
                                .send(UiCommand::ShowWarning(format!(
                                    "Warning: Failed to persist to audit log: {e}"
                                )))
                                .await;
                        } else {
                            debug!(seq = seq, "Prompt record persisted");
                        }

                        send_record_to_ui(&commands, seq, &tx, &prompt_entry).await;
                        seq += 1;

                        messages = result.messages.clone();

                        let response_tx = create_response_transaction(agent_id, &result.total_text);

                        if let Err(e) = store.enqueue_tx(&response_tx) {
                            error!(error = %e, "Failed to enqueue response transaction");
                        } else if let Ok(Some((resp_inbox_seq, resp_tx))) = store.dequeue_tx(agent_id) {
                            let response_context_hash = compute_context_hash(seq, &resp_tx);
                            let response_entry = RecordEntry::builder(seq, resp_tx.clone())
                                .context_hash(response_context_hash)
                                .build();

                            if let Err(e) = store.append_entry_atomic(
                                agent_id,
                                seq,
                                &response_entry,
                                resp_inbox_seq,
                            ) {
                                error!(error = %e, "Failed to persist response record");
                            } else {
                                debug!(seq = seq, "Response record persisted");
                            }

                            send_record_to_ui(&commands, seq, &resp_tx, &response_entry).await;
                            seq += 1;
                        }

                        if !result.total_text.is_empty() {
                            let preview: String = result.total_text.chars().take(100).collect();
                            info!(response_preview = %preview, "Model response received");

                            let _ = commands
                                .send(UiCommand::ShowMessage(aura_terminal::events::MessageData {
                                    role: aura_terminal::events::MessageRole::Assistant,
                                    content: result.total_text.clone(),
                                    is_streaming: false,
                                }))
                                .await;
                        }

                        if let Some(ref err) = result.llm_error {
                            let _ = commands
                                .send(UiCommand::ShowWarning(format!("LLM error: {err}")))
                                .await;
                        }

                        if result.timed_out {
                            let _ = commands
                                .send(UiCommand::ShowWarning("Agent loop timed out".to_string()))
                                .await;
                        }

                        let _ = commands.send(UiCommand::Complete).await;
                    }
                    Err(e) => {
                        error!(error = %e, "Agent loop failed");
                        let _ = commands
                            .send(UiCommand::ShowError(format!("Error: {e}")))
                            .await;
                        let _ = commands.send(UiCommand::Complete).await;
                    }
                }
            }
            UiEvent::Approve(_id) => {
                debug!("Approval received");
            }
            UiEvent::Deny(_id) => {
                debug!("Denial received");
            }
            UiEvent::Quit => {
                debug!("Quit received");
                break;
            }
            UiEvent::Cancel => {
                debug!("Cancel received");
                let _ = commands
                    .send(UiCommand::SetStatus("Cancelled".to_string()))
                    .await;
            }
            UiEvent::ShowStatus => {
            }
            UiEvent::ShowHelp => {
            }
            UiEvent::ShowHistory(_) => {
            }
            UiEvent::Clear => {
                let _ = commands.send(UiCommand::ClearConversation).await;
            }
            UiEvent::NewSession => {
                debug!("New session requested, seq={}", seq);

                let session_tx = Transaction::session_start(agent_id);

                if let Err(e) = store.enqueue_tx(&session_tx) {
                    error!(error = %e, "Failed to enqueue session start");
                } else if let Ok(Some((inbox_seq, tx))) = store.dequeue_tx(agent_id) {
                    let context_hash = compute_context_hash(seq, &tx);
                    let entry = RecordEntry::builder(seq, tx.clone())
                        .context_hash(context_hash)
                        .build();

                    if let Err(e) = store.append_entry_atomic(agent_id, seq, &entry, inbox_seq) {
                        error!(error = %e, "Failed to persist session start record");
                    } else {
                        debug!(seq = seq, "Session start record persisted");
                        send_record_to_ui(&commands, seq, &tx, &entry).await;
                        seq += 1;
                    }
                }

                messages.clear();

                let _ = commands
                    .send(UiCommand::SetStatus("Ready".to_string()))
                    .await;
            }
            UiEvent::SelectAgent(_agent_id) => {
                debug!("Agent selection not yet implemented");
            }
            UiEvent::RefreshAgents => {
                debug!("Agent refresh not yet implemented");
            }
        } // end match event
            } // end Some(event) arm
        } // end tokio::select!
    } // end loop

    #[allow(unreachable_code)]
    Ok(())
}

/// Send a record summary to the UI (matching the stored format).
async fn send_record_to_ui(
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
fn compute_context_hash(seq: u64, tx: &Transaction) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&seq.to_be_bytes());
    hasher.update(tx.hash.as_bytes());
    hasher.update(&tx.ts_ms.to_be_bytes());
    hasher.update(&tx.payload);
    *hasher.finalize().as_bytes()
}

/// Create a response transaction for the assistant's message.
fn create_response_transaction(agent_id: AgentId, response_text: &str) -> Transaction {
    Transaction::new_chained(
        agent_id,
        TransactionType::AgentMsg,
        response_text.as_bytes().to_vec(),
        None,
    )
}
