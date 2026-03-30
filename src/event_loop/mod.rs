//! Event processing loop for the terminal UI mode.

mod agent_events;
mod record_ui;

use agent_events::forward_agent_events;
use record_ui::{compute_context_hash, create_response_transaction, send_record_to_ui};

use aura_agent::{AgentLoop, AgentLoopEvent, KernelToolExecutor, ProcessManager};
use aura_core::{AgentId, RecordEntry, Transaction};
use aura_reasoner::{Message, ModelProvider, ToolDefinition};
use aura_store::{RocksStore, Store};
use aura_terminal::{UiCommand, UiEvent};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

const TURN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Bundled dependencies for the event loop, reducing parameter count.
pub struct EventLoopContext<'a> {
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

/// Mutable state threaded through all event handlers.
struct LoopState<'a> {
    seq: u64,
    messages: Vec<Message>,
    commands: &'a mpsc::Sender<UiCommand>,
    agent_loop: &'a mut AgentLoop,
    provider: &'a dyn ModelProvider,
    executor: &'a KernelToolExecutor,
    tools: &'a [ToolDefinition],
    store: Arc<RocksStore>,
    agent_id: AgentId,
}

/// Run the event processing loop.
///
/// Handles user messages from the UI and process completion events.
pub async fn run_event_loop(ctx: EventLoopContext<'_>) -> anyhow::Result<()> {
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

    let mut state = LoopState {
        seq: store.get_head_seq(agent_id).unwrap_or(0) + 1,
        messages: Vec::new(),
        commands: &commands,
        agent_loop,
        provider,
        executor,
        tools,
        store,
        agent_id,
    };

    loop {
        tokio::select! {
            Some(completion_tx) = process_completions.recv() => {
                handle_completion(&mut state, completion_tx).await;
            }
            Some(event) = events.recv() => {
                if handle_ui_event(&mut state, event).await {
                    break;
                }
            }
        }
    }

    #[allow(unreachable_code)]
    Ok(())
}

async fn handle_completion(state: &mut LoopState<'_>, completion_tx: Transaction) {
    info!(
        hash = %completion_tx.hash,
        tx_type = ?completion_tx.tx_type,
        "Processing async process completion"
    );

    if let Err(e) = state.store.enqueue_tx(&completion_tx) {
        error!(error = %e, "Failed to enqueue completion transaction");
        return;
    }

    if let Ok(Some((inbox_seq, tx))) = state.store.dequeue_tx(state.agent_id) {
        let context_hash = compute_context_hash(state.seq, &tx);
        let entry = RecordEntry::builder(state.seq, tx.clone())
            .context_hash(context_hash)
            .build();

        if let Err(e) =
            state
                .store
                .append_entry_atomic(state.agent_id, state.seq, &entry, inbox_seq)
        {
            error!(error = %e, "Failed to persist completion record");
        } else {
            debug!(seq = state.seq, "Completion record persisted");
            send_record_to_ui(state.commands, state.seq, &tx, &entry).await;
            state.seq += 1;

            let _ = state
                .commands
                .send(UiCommand::SetStatus("Process completed".to_string()))
                .await;
        }
    }
}

/// Returns `true` if the loop should break (quit).
async fn handle_ui_event(state: &mut LoopState<'_>, event: UiEvent) -> bool {
    match event {
        UiEvent::UserMessage(text) => {
            handle_user_message(state, text).await;
        }
        UiEvent::Approve(_id) => debug!("Approval received"),
        UiEvent::Deny(_id) => debug!("Denial received"),
        UiEvent::Quit => {
            debug!("Quit received");
            return true;
        }
        UiEvent::Cancel => {
            debug!("Cancel received");
            let _ = state
                .commands
                .send(UiCommand::SetStatus("Cancelled".to_string()))
                .await;
        }
        UiEvent::ShowStatus | UiEvent::ShowHelp | UiEvent::ShowHistory(_) => {}
        UiEvent::Clear => {
            let _ = state.commands.send(UiCommand::ClearConversation).await;
        }
        UiEvent::NewSession => handle_new_session(state).await,
        UiEvent::SelectAgent(_) => debug!("Agent selection not yet implemented"),
        UiEvent::RefreshAgents => debug!("Agent refresh not yet implemented"),
        UiEvent::LoginCredentials { email, password } => {
            handle_login(state, &email, &password).await;
        }
        UiEvent::Logout => handle_logout(state).await,
        UiEvent::Whoami => handle_whoami(state).await,
    }
    false
}

async fn handle_user_message(state: &mut LoopState<'_>, text: String) {
    info!(text = %text, seq = state.seq, "Processing user message");

    let _ = state
        .commands
        .send(UiCommand::SetStatus("Thinking...".to_string()))
        .await;

    drain_stale_inbox(state).await;

    let (tx, inbox_seq) = match enqueue_and_dequeue(state, &text).await {
        Some(v) => v,
        None => return,
    };

    state.messages.push(Message::user(text));

    let (process_result, streamed_text) = run_agent_turn(state, &tx, inbox_seq).await;

    match process_result {
        Ok(result) => {
            handle_agent_success(state, result, &tx, inbox_seq, streamed_text).await;
        }
        Err(e) => {
            error!(error = %e, "Agent loop failed");
            let _ = state
                .commands
                .send(UiCommand::ShowError(format!("Error: {e}")))
                .await;
            let _ = state.commands.send(UiCommand::Complete).await;
        }
    }
}

async fn drain_stale_inbox(state: &mut LoopState<'_>) {
    let mut stale_count = 0;
    while let Ok(Some((stale_inbox_seq, stale_tx))) = state.store.dequeue_tx(state.agent_id) {
        warn!(
            stale_inbox_seq = stale_inbox_seq,
            stale_tx_type = ?stale_tx.tx_type,
            "Discarding stale inbox transaction"
        );
        let stale_entry = RecordEntry::builder(state.seq, stale_tx.clone())
            .context_hash(compute_context_hash(state.seq, &stale_tx))
            .build();
        if let Err(e) = state.store.append_entry_atomic(
            state.agent_id,
            state.seq,
            &stale_entry,
            stale_inbox_seq,
        ) {
            error!(error = %e, "Failed to clear stale transaction");
            break;
        }
        state.seq += 1;
        stale_count += 1;
        if stale_count > 10 {
            error!("Too many stale transactions, aborting drain");
            break;
        }
    }
}

async fn enqueue_and_dequeue(state: &mut LoopState<'_>, text: &str) -> Option<(Transaction, u64)> {
    let tx = Transaction::user_prompt(state.agent_id, text.to_string());
    if let Err(e) = state.store.enqueue_tx(&tx) {
        error!(error = %e, "Failed to enqueue transaction");
        let _ = state
            .commands
            .send(UiCommand::ShowError(format!("Storage error: {e}")))
            .await;
        let _ = state.commands.send(UiCommand::Complete).await;
        return None;
    }

    let (inbox_seq, dequeued_tx) = match state.store.dequeue_tx(state.agent_id) {
        Ok(Some(item)) => item,
        Ok(None) => {
            error!("Transaction was enqueued but not found in inbox");
            return None;
        }
        Err(e) => {
            error!(error = %e, "Failed to dequeue transaction");
            return None;
        }
    };

    if dequeued_tx.hash != tx.hash {
        error!("Transaction mismatch after draining stale entries");
    }

    Some((tx, inbox_seq))
}

async fn run_agent_turn(
    state: &mut LoopState<'_>,
    _tx: &Transaction,
    _inbox_seq: u64,
) -> (
    Result<aura_agent::AgentLoopResult, aura_agent::AgentError>,
    bool,
) {
    let (agent_event_tx, agent_event_rx) = tokio::sync::mpsc::channel::<AgentLoopEvent>(1024);

    let fwd_commands = state.commands.clone();
    let forwarder = tokio::spawn(forward_agent_events(agent_event_rx, fwd_commands));

    let cancel_token = CancellationToken::new();
    let cancel_for_timeout = cancel_token.clone();

    let process_result = match tokio::time::timeout(
        TURN_TIMEOUT,
        state.agent_loop.run_with_events(
            state.provider,
            state.executor,
            state.messages.clone(),
            state.tools.to_vec(),
            Some(agent_event_tx),
            Some(cancel_token),
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            cancel_for_timeout.cancel();
            Err(aura_agent::AgentError::Timeout(format!(
                "Agent turn timed out after {} minutes",
                TURN_TIMEOUT.as_secs() / 60
            )))
        }
    };

    let streamed_text = match forwarder.await {
        Ok(fwd_state) => {
            if fwd_state.thinking_active {
                let _ = state.commands.send(UiCommand::FinishThinking).await;
            }
            if fwd_state.streaming_active {
                let _ = state.commands.send(UiCommand::FinishStreaming).await;
            }
            fwd_state.had_text
        }
        Err(e) => {
            warn!(error = %e, "Event forwarder panicked");
            false
        }
    };

    (process_result, streamed_text)
}

async fn handle_agent_success(
    state: &mut LoopState<'_>,
    result: aura_agent::AgentLoopResult,
    tx: &Transaction,
    inbox_seq: u64,
    streamed_text: bool,
) {
    persist_prompt_record(state, tx, inbox_seq).await;
    state.messages = result.messages.clone();
    persist_response_record(state, &result.total_text).await;
    emit_response_to_ui(state, &result, streamed_text).await;

    if let Some(ref err) = result.llm_error {
        let _ = state
            .commands
            .send(UiCommand::ShowWarning(format!("LLM error: {err}")))
            .await;
    }
    if result.timed_out {
        let _ = state
            .commands
            .send(UiCommand::ShowWarning("Agent loop timed out".to_string()))
            .await;
    }
    let _ = state.commands.send(UiCommand::Complete).await;
}

async fn persist_prompt_record(state: &mut LoopState<'_>, tx: &Transaction, inbox_seq: u64) {
    let context_hash = compute_context_hash(state.seq, tx);
    let entry = RecordEntry::builder(state.seq, tx.clone())
        .context_hash(context_hash)
        .build();

    if let Err(e) = state
        .store
        .append_entry_atomic(state.agent_id, state.seq, &entry, inbox_seq)
    {
        error!(error = %e, "Failed to persist prompt record");
        let _ = state
            .commands
            .send(UiCommand::ShowWarning(format!(
                "Warning: Failed to persist to audit log: {e}"
            )))
            .await;
    } else {
        debug!(seq = state.seq, "Prompt record persisted");
    }

    send_record_to_ui(state.commands, state.seq, tx, &entry).await;
    state.seq += 1;
}

async fn persist_response_record(state: &mut LoopState<'_>, total_text: &str) {
    let response_tx = create_response_transaction(state.agent_id, total_text);

    if let Err(e) = state.store.enqueue_tx(&response_tx) {
        error!(error = %e, "Failed to enqueue response transaction");
        return;
    }

    if let Ok(Some((resp_inbox_seq, resp_tx))) = state.store.dequeue_tx(state.agent_id) {
        let context_hash = compute_context_hash(state.seq, &resp_tx);
        let entry = RecordEntry::builder(state.seq, resp_tx.clone())
            .context_hash(context_hash)
            .build();

        if let Err(e) =
            state
                .store
                .append_entry_atomic(state.agent_id, state.seq, &entry, resp_inbox_seq)
        {
            error!(error = %e, "Failed to persist response record");
        } else {
            debug!(seq = state.seq, "Response record persisted");
        }

        send_record_to_ui(state.commands, state.seq, &resp_tx, &entry).await;
        state.seq += 1;
    }
}

async fn emit_response_to_ui(
    state: &LoopState<'_>,
    result: &aura_agent::AgentLoopResult,
    streamed_text: bool,
) {
    if !result.total_text.is_empty() {
        let preview: String = result.total_text.chars().take(100).collect();
        info!(response_preview = %preview, "Model response received");

        if !streamed_text {
            let _ = state
                .commands
                .send(UiCommand::ShowMessage(aura_terminal::events::MessageData {
                    role: aura_terminal::events::MessageRole::Assistant,
                    content: result.total_text.clone(),
                    is_streaming: false,
                }))
                .await;
        }
    }
}

async fn handle_new_session(state: &mut LoopState<'_>) {
    debug!("New session requested, seq={}", state.seq);

    let session_tx = Transaction::session_start(state.agent_id);

    if let Err(e) = state.store.enqueue_tx(&session_tx) {
        error!(error = %e, "Failed to enqueue session start");
    } else if let Ok(Some((inbox_seq, tx))) = state.store.dequeue_tx(state.agent_id) {
        let context_hash = compute_context_hash(state.seq, &tx);
        let entry = RecordEntry::builder(state.seq, tx.clone())
            .context_hash(context_hash)
            .build();

        if let Err(e) =
            state
                .store
                .append_entry_atomic(state.agent_id, state.seq, &entry, inbox_seq)
        {
            error!(error = %e, "Failed to persist session start record");
        } else {
            debug!(seq = state.seq, "Session start record persisted");
            send_record_to_ui(state.commands, state.seq, &tx, &entry).await;
            state.seq += 1;
        }
    }

    state.messages.clear();

    let _ = state
        .commands
        .send(UiCommand::SetStatus("Ready".to_string()))
        .await;
}

async fn handle_login(state: &mut LoopState<'_>, email: &str, password: &str) {
    let _ = state
        .commands
        .send(UiCommand::SetStatus("Authenticating...".to_string()))
        .await;
    match aura_auth::ZosClient::new() {
        Ok(client) => match client.login(email, password).await {
            Ok(stored) => {
                let display = stored.display_name.clone();
                let zid = stored.primary_zid.clone();
                let token = stored.access_token.clone();
                if let Err(e) = aura_auth::CredentialStore::save(&stored) {
                    let _ = state
                        .commands
                        .send(UiCommand::ShowError(format!(
                            "Failed to save credentials: {e}"
                        )))
                        .await;
                } else {
                    state.agent_loop.set_auth_token(Some(token));
                    let _ = state
                        .commands
                        .send(UiCommand::ShowSuccess(format!(
                            "Logged in as {display} ({zid})"
                        )))
                        .await;
                }
            }
            Err(e) => {
                let _ = state
                    .commands
                    .send(UiCommand::ShowError(format!("Login failed: {e}")))
                    .await;
            }
        },
        Err(e) => {
            let _ = state
                .commands
                .send(UiCommand::ShowError(format!("Auth client error: {e}")))
                .await;
        }
    }
    let _ = state.commands.send(UiCommand::Complete).await;
}

async fn handle_logout(state: &mut LoopState<'_>) {
    if let Some(stored) = aura_auth::CredentialStore::load() {
        if let Ok(client) = aura_auth::ZosClient::new() {
            client.logout(&stored.access_token).await;
        }
    }
    match aura_auth::CredentialStore::clear() {
        Ok(()) => {
            state.agent_loop.set_auth_token(None);
            let _ = state
                .commands
                .send(UiCommand::ShowSuccess("Logged out".to_string()))
                .await;
        }
        Err(e) => {
            let _ = state
                .commands
                .send(UiCommand::ShowError(format!(
                    "Failed to clear credentials: {e}"
                )))
                .await;
        }
    }
}

async fn handle_whoami(state: &LoopState<'_>) {
    match aura_auth::CredentialStore::load() {
        Some(session) => {
            let msg = format!(
                "Logged in as {} (zID: {}, User: {}, Since: {})",
                session.display_name,
                session.primary_zid,
                session.user_id,
                session.created_at.format("%Y-%m-%d %H:%M UTC"),
            );
            let _ = state
                .commands
                .send(UiCommand::ShowMessage(aura_terminal::events::MessageData {
                    role: aura_terminal::events::MessageRole::System,
                    content: msg,
                    is_streaming: false,
                }))
                .await;
        }
        None => {
            let _ = state
                .commands
                .send(UiCommand::ShowMessage(aura_terminal::events::MessageData {
                    role: aura_terminal::events::MessageRole::System,
                    content: "Not logged in. Use /login to authenticate.".to_string(),
                    is_streaming: false,
                }))
                .await;
        }
    }
}
