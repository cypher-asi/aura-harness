//! Handler functions for agent turns, auth, and session lifecycle.

use super::record_ui::{create_response_transaction, send_record_to_ui};
use super::{forward_agent_events, LoopState, TURN_TIMEOUT};
use aura_agent::AgentLoopEvent;
use aura_core_types::Transaction;
use aura_terminal::UiCommand;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Run one turn of the active agent on `agent_loop` with `tools`.
pub(super) async fn run_agent_turn(
    state: &mut LoopState<'_>,
    agent_loop: &aura_agent::AgentLoop,
    tools: Vec<aura_model_reasoner::ToolDefinition>,
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
        agent_loop.run_with_events(
            &state.session.model_gateway,
            &state.session.tool_gateway,
            state.session.messages.clone(),
            tools,
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
    streamed_text: bool,
) {
    state.session.messages = result.messages.clone();

    persist_response_via_kernel(state, &result.total_text).await;
    emit_response_to_ui(state, &result, streamed_text).await;

    // Memory ingestion now fires automatically via TurnObserver inside
    // AgentLoop::run_with_events — no manual call needed here.

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

async fn persist_response_via_kernel(state: &mut LoopState<'_>, total_text: &str) {
    let response_tx = create_response_transaction(state.session.agent_id(), total_text);

    match state
        .session
        .kernel
        .process_direct(response_tx.clone())
        .await
    {
        Ok(result) => {
            debug!(
                seq = result.entry.seq,
                "Response record persisted via kernel"
            );
            send_record_to_ui(
                state.commands,
                result.entry.seq,
                &response_tx,
                &result.entry,
            )
            .await;
        }
        Err(e) => {
            error!(error = %e, "Failed to persist response record via kernel");
        }
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
                    timestamp_ms: None,
                    command: None,
                    activity: None,
                }))
                .await;
        }
    }
}

pub(super) async fn handle_new_session(state: &mut LoopState<'_>) {
    debug!("New session requested");

    let session_tx = Transaction::session_start(state.session.agent_id());
    let store = state.session.kernel.store();

    if let Err(e) = store.enqueue_tx(&session_tx) {
        error!(error = %e, "Failed to enqueue session start");
    } else if let Ok(Some((token, tx))) = store.dequeue_tx(state.session.agent_id()) {
        match state
            .session
            .kernel
            .process_dequeued(tx.clone(), token)
            .await
        {
            Ok(result) => {
                debug!(
                    seq = result.entry.seq,
                    "Session start record persisted via kernel"
                );
                send_record_to_ui(state.commands, result.entry.seq, &tx, &result.entry).await;
            }
            Err(e) => {
                error!(error = %e, "Failed to persist session start record via kernel");
            }
        }
    }

    state.session.messages.clear();

    let _ = state
        .commands
        .send(UiCommand::SetStatus("Ready".to_string()))
        .await;
}

pub(super) async fn show_system_message(state: &LoopState<'_>, content: String) {
    let _ = state
        .commands
        .send(UiCommand::ShowMessage(aura_terminal::events::MessageData {
            role: aura_terminal::events::MessageRole::System,
            content,
            is_streaming: false,
            timestamp_ms: None,
            command: None,
            activity: None,
        }))
        .await;
}

/// Probe the local model server, publish the model list, and explain how
/// to get the local default running when it isn't.
pub(super) async fn announce_models(state: &mut LoopState<'_>) {
    state.models.refresh_local().await;
    let _ = state.commands.send(state.models.ui_command()).await;
    if let Some(hint) = state.models.local_hint() {
        show_system_message(state, hint).await;
    }
}

pub(super) async fn handle_select_model(state: &mut LoopState<'_>, id: &str) {
    if aura_model_reasoner::model_catalog::backend_for(id)
        == aura_model_reasoner::model_catalog::ModelBackend::Local
    {
        state.models.refresh_local().await;
    }
    match state.models.select(state.agent_loop.config_mut(), id) {
        Ok(()) => {
            info!(model = %state.models.current(), "Model selected");
            let _ = state
                .commands
                .send(UiCommand::ShowSuccess(format!(
                    "Model: {}",
                    crate::model_selection::ModelSelection::label(state.models.current())
                )))
                .await;
            if let Some(hint) = state.models.local_hint() {
                show_system_message(state, hint).await;
            }
        }
        Err(problem) => {
            let _ = state.commands.send(UiCommand::ShowWarning(problem)).await;
        }
    }
    let _ = state.commands.send(state.models.ui_command()).await;
}
