//! Event processing loop for the terminal UI mode.

use crate::record_loader::extract_tool_info;
use aura_agent::{AgentLoop, AgentLoopEvent, KernelToolExecutor};
use aura_core::{AgentId, EffectStatus, RecordEntry, Transaction, TransactionType};
use aura_runtime::ProcessManager;
use aura_reasoner::{Message, ModelProvider, ToolDefinition};
use aura_store::{RocksStore, Store};
use aura_terminal::{
    events::{RecordStatus, RecordSummary, ToolData},
    UiCommand, UiEvent,
};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

const TURN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Bundled dependencies for the event loop, reducing parameter count.
pub(crate) struct EventLoopContext<'a> {
    pub events: &'a mut mpsc::Receiver<UiEvent>,
    pub process_completions: mpsc::Receiver<Transaction>,
    pub commands: mpsc::Sender<UiCommand>,
    pub agent_loop: &'a mut AgentLoop,
    pub provider: &'a dyn ModelProvider,
    pub executor: &'a KernelToolExecutor,
    pub tools: &'a [ToolDefinition],
    pub store: Arc<RocksStore>,
    pub agent_id: AgentId,
    pub _process_manager: Arc<ProcessManager>,
}

/// Run the event processing loop.
///
/// Handles user messages from the UI and process completion events.
pub(crate) async fn run_event_loop(ctx: EventLoopContext<'_>) -> anyhow::Result<()> {
    let EventLoopContext {
        events,
        mut process_completions,
        commands,
        agent_loop,
        provider,
        executor,
        tools,
        store,
        agent_id,
        _process_manager,
    } = ctx;
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

                let (agent_event_tx, agent_event_rx) =
                    tokio::sync::mpsc::unbounded_channel::<AgentLoopEvent>();

                let fwd_commands = commands.clone();
                let forwarder = tokio::spawn(
                    forward_agent_events(agent_event_rx, fwd_commands),
                );

                let cancel_token = CancellationToken::new();
                let cancel_for_timeout = cancel_token.clone();

                let process_result = match tokio::time::timeout(
                    TURN_TIMEOUT,
                    agent_loop.run_with_events(
                        provider,
                        executor,
                        messages.clone(),
                        tools.to_vec(),
                        Some(agent_event_tx),
                        Some(cancel_token),
                    ),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        cancel_for_timeout.cancel();
                        Err(aura_agent::AgentError::Timeout(format!("Agent turn timed out after {} minutes", TURN_TIMEOUT.as_secs() / 60)))
                    }
                };

                let streamed_text = match forwarder.await {
                    Ok(state) => {
                        if state.thinking_active {
                            let _ = commands.send(UiCommand::FinishThinking).await;
                        }
                        if state.streaming_active {
                            let _ = commands.send(UiCommand::FinishStreaming).await;
                        }
                        state.had_text
                    }
                    Err(e) => {
                        warn!(error = %e, "Event forwarder panicked");
                        false
                    }
                };

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

                            if !streamed_text {
                                let _ = commands
                                    .send(UiCommand::ShowMessage(aura_terminal::events::MessageData {
                                        role: aura_terminal::events::MessageRole::Assistant,
                                        content: result.total_text.clone(),
                                        is_streaming: false,
                                    }))
                                    .await;
                            }
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
            UiEvent::LoginCredentials { email, password } => {
                let _ = commands.send(UiCommand::SetStatus("Authenticating...".to_string())).await;
                match aura_auth::ZosClient::new() {
                    Ok(client) => match client.login(&email, &password).await {
                        Ok(stored) => {
                            let display = stored.display_name.clone();
                            let zid = stored.primary_zid.clone();
                            let token = stored.access_token.clone();
                            if let Err(e) = aura_auth::CredentialStore::save(&stored) {
                                let _ = commands.send(UiCommand::ShowError(format!("Failed to save credentials: {e}"))).await;
                            } else {
                                agent_loop.set_auth_token(Some(token));
                                let _ = commands.send(UiCommand::ShowSuccess(format!("Logged in as {display} ({zid})"))).await;
                            }
                        }
                        Err(e) => {
                            let _ = commands.send(UiCommand::ShowError(format!("Login failed: {e}"))).await;
                        }
                    },
                    Err(e) => {
                        let _ = commands.send(UiCommand::ShowError(format!("Auth client error: {e}"))).await;
                    }
                }
                let _ = commands.send(UiCommand::Complete).await;
            }
            UiEvent::Logout => {
                if let Some(stored) = aura_auth::CredentialStore::load() {
                    if let Ok(client) = aura_auth::ZosClient::new() {
                        client.logout(&stored.access_token).await;
                    }
                }
                match aura_auth::CredentialStore::clear() {
                    Ok(()) => {
                        agent_loop.set_auth_token(None);
                        let _ = commands.send(UiCommand::ShowSuccess("Logged out".to_string())).await;
                    }
                    Err(e) => {
                        let _ = commands.send(UiCommand::ShowError(format!("Failed to clear credentials: {e}"))).await;
                    }
                }
            }
            UiEvent::Whoami => {
                match aura_auth::CredentialStore::load() {
                    Some(session) => {
                        let msg = format!(
                            "Logged in as {} (zID: {}, User: {}, Since: {})",
                            session.display_name,
                            session.primary_zid,
                            session.user_id,
                            session.created_at.format("%Y-%m-%d %H:%M UTC"),
                        );
                        let _ = commands
                            .send(UiCommand::ShowMessage(aura_terminal::events::MessageData {
                                role: aura_terminal::events::MessageRole::System,
                                content: msg,
                                is_streaming: false,
                            }))
                            .await;
                    }
                    None => {
                        let _ = commands
                            .send(UiCommand::ShowMessage(aura_terminal::events::MessageData {
                                role: aura_terminal::events::MessageRole::System,
                                content: "Not logged in. Use /login to authenticate.".to_string(),
                                is_streaming: false,
                            }))
                            .await;
                    }
                }
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

// ---------------------------------------------------------------------------
// Streaming event forwarder
// ---------------------------------------------------------------------------

/// Tracks forwarder lifecycle so the caller can finalize streaming/thinking.
struct ForwarderState {
    streaming_active: bool,
    thinking_active: bool,
    had_text: bool,
}

/// Reads [`AgentLoopEvent`]s and translates them into [`UiCommand`]s.
async fn forward_agent_events(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<AgentLoopEvent>,
    commands: mpsc::Sender<UiCommand>,
) -> ForwarderState {
    let mut state = ForwarderState {
        streaming_active: false,
        thinking_active: false,
        had_text: false,
    };

    while let Some(event) = rx.recv().await {
        match event {
            AgentLoopEvent::ThinkingDelta(text) => {
                if !state.thinking_active {
                    let _ = commands.send(UiCommand::StartThinking).await;
                    state.thinking_active = true;
                }
                let _ = commands.send(UiCommand::AppendThinking(text)).await;
            }
            AgentLoopEvent::TextDelta(text) => {
                if state.thinking_active {
                    let _ = commands.send(UiCommand::FinishThinking).await;
                    state.thinking_active = false;
                }
                if !state.streaming_active {
                    let _ = commands.send(UiCommand::StartStreaming).await;
                    state.streaming_active = true;
                }
                state.had_text = true;
                let _ = commands.send(UiCommand::AppendText(text)).await;
            }
            AgentLoopEvent::ToolStart { id, name } => {
                if state.thinking_active {
                    let _ = commands.send(UiCommand::FinishThinking).await;
                    state.thinking_active = false;
                }
                let _ = commands
                    .send(UiCommand::ShowTool(ToolData {
                        id,
                        name,
                        args: String::new(),
                    }))
                    .await;
            }
            AgentLoopEvent::ToolInputSnapshot { id, .. } => {
                debug!(tool_id = %id, "Tool input streaming");
            }
            AgentLoopEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => {
                let _ = commands
                    .send(UiCommand::CompleteTool {
                        id: tool_use_id,
                        result: content,
                        success: !is_error,
                    })
                    .await;
            }
            AgentLoopEvent::IterationComplete { .. } => {}
            AgentLoopEvent::Warning(msg) => {
                let _ = commands.send(UiCommand::ShowWarning(msg)).await;
            }
            AgentLoopEvent::Error { message, .. } => {
                let _ = commands.send(UiCommand::ShowWarning(message)).await;
            }
            AgentLoopEvent::ToolComplete { .. } => {}
        }
    }

    state
}
