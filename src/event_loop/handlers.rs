//! Handler functions for agent turns, auth, and session lifecycle.

use super::record_ui::{compute_context_hash, create_response_transaction, send_record_to_ui};
use super::{forward_agent_events, LoopState, TURN_TIMEOUT};
use aura_agent::AgentLoopEvent;
use aura_core::{RecordEntry, Transaction};
use aura_store::Store;
use aura_terminal::UiCommand;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

pub(super) async fn run_agent_turn(
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

pub(super) async fn handle_agent_success(
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

    if let Ok(Some((resp_token, resp_tx))) = state.store.dequeue_tx(state.agent_id) {
        let context_hash = compute_context_hash(state.seq, &resp_tx);
        let entry = RecordEntry::builder(state.seq, resp_tx.clone())
            .context_hash(context_hash)
            .build();

        if let Err(e) =
            state
                .store
                .append_entry_atomic(state.agent_id, state.seq, &entry, resp_token.inbox_seq())
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

pub(super) async fn handle_new_session(state: &mut LoopState<'_>) {
    debug!("New session requested, seq={}", state.seq);

    let session_tx = Transaction::session_start(state.agent_id);

    if let Err(e) = state.store.enqueue_tx(&session_tx) {
        error!(error = %e, "Failed to enqueue session start");
    } else if let Ok(Some((token, tx))) = state.store.dequeue_tx(state.agent_id) {
        let context_hash = compute_context_hash(state.seq, &tx);
        let entry = RecordEntry::builder(state.seq, tx.clone())
            .context_hash(context_hash)
            .build();

        if let Err(e) =
            state
                .store
                .append_entry_atomic(state.agent_id, state.seq, &entry, token.inbox_seq())
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

pub(super) async fn handle_login(state: &mut LoopState<'_>, email: &str, password: &str) {
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

pub(super) async fn handle_logout(state: &mut LoopState<'_>) {
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

pub(super) async fn handle_whoami(state: &LoopState<'_>) {
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
