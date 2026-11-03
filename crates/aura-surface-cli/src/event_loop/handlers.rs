//! Handler functions for agent turns, auth, and session lifecycle.

use super::agent_events::{AgentUi, ForwarderState};
use super::agents::AgentSession;
use super::record_ui::{create_response_transaction, send_record_to_ui};
use super::{forward_agent_events, LoopState, TURN_TIMEOUT};
use aura_agent::AgentLoopEvent;
use aura_core_types::Transaction;
use aura_terminal::events::{MessageData, MessageRole};
use aura_terminal::UiCommand;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Run one turn of `session` on `agent_loop` with `tools`, streaming it
/// into `ui`. `cancel` stops it early.
pub(super) async fn run_agent_turn(
    session: &AgentSession,
    agent_loop: &aura_agent::AgentLoop,
    tools: Vec<aura_model_reasoner::ToolDefinition>,
    ui: &AgentUi,
    cancel: &CancellationToken,
) -> (
    Result<aura_agent::AgentLoopResult, aura_agent::AgentError>,
    ForwarderState,
) {
    let (agent_event_tx, agent_event_rx) = tokio::sync::mpsc::channel::<AgentLoopEvent>(1024);
    let forwarder = tokio::spawn(forward_agent_events(agent_event_rx, ui.clone()));

    // A timeout stops the turn without it counting as cancelled by the user.
    let turn_cancel = cancel.child_token();
    let process_result = match tokio::time::timeout(
        TURN_TIMEOUT,
        agent_loop.run_with_events(
            &session.model_gateway,
            &session.tool_gateway,
            session.messages.clone(),
            tools,
            Some(agent_event_tx),
            Some(turn_cancel.clone()),
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            turn_cancel.cancel();
            Err(aura_agent::AgentError::Timeout(format!(
                "Agent turn timed out after {} minutes",
                TURN_TIMEOUT.as_secs() / 60
            )))
        }
    };

    let forwarded = match forwarder.await {
        Ok(fwd_state) => {
            if fwd_state.thinking_active {
                ui.send(UiCommand::FinishThinking).await;
            }
            if fwd_state.streaming_active {
                ui.send(UiCommand::FinishStreaming).await;
            }
            fwd_state
        }
        Err(e) => {
            warn!(error = %e, "Event forwarder panicked");
            ForwarderState::default()
        }
    };

    (process_result, forwarded)
}

pub(super) async fn handle_agent_success(
    session: &mut AgentSession,
    result: aura_agent::AgentLoopResult,
    forwarded: &ForwarderState,
    ui: &AgentUi,
) {
    session.messages.clone_from(&result.messages);

    persist_response_via_kernel(session, &result.total_text, ui).await;
    emit_response_to_ui(&result, forwarded, ui).await;

    // Memory ingestion now fires automatically via TurnObserver inside
    // AgentLoop::run_with_events — no manual call needed here.

    if let Some(ref err) = result.llm_error {
        ui.send(UiCommand::ShowWarning(format!("LLM error: {err}")))
            .await;
    }
    if result.timed_out {
        ui.send(UiCommand::ShowWarning("Agent loop timed out".to_string()))
            .await;
    }
    ui.send(UiCommand::Complete).await;
}

async fn persist_response_via_kernel(session: &AgentSession, total_text: &str, ui: &AgentUi) {
    let response_tx = create_response_transaction(session.agent_id(), total_text);

    match session.kernel.process_direct(response_tx.clone()).await {
        Ok(result) => {
            debug!(
                seq = result.entry.seq,
                "Response record persisted via kernel"
            );
            send_record_to_ui(ui.commands(), result.entry.seq, &response_tx, &result.entry).await;
        }
        Err(e) => {
            error!(error = %e, "Failed to persist response record via kernel");
        }
    }
}

async fn emit_response_to_ui(
    result: &aura_agent::AgentLoopResult,
    forwarded: &ForwarderState,
    ui: &AgentUi,
) {
    if !result.total_text.is_empty() {
        let preview: String = result.total_text.chars().take(100).collect();
        info!(response_preview = %preview, "Model response received");

        if !forwarded.had_text {
            ui.send(UiCommand::ShowMessage(MessageData {
                role: MessageRole::Assistant,
                content: result.total_text.clone(),
                is_streaming: false,
                timestamp_ms: None,
                command: None,
                activity: None,
                model: forwarded.reply_model.clone(),
                usage: forwarded.reply_usage,
            }))
            .await;
        }
    }
}

/// Start a new session on the active agent. Returns `false`, after
/// saying why, while the agent is busy.
pub(super) async fn handle_new_session(state: &mut LoopState<'_>) -> bool {
    debug!("New session requested");

    let agent_id = state.active;
    let Some(session) = state.idle.get_mut(&agent_id) else {
        let _ = state
            .commands
            .send(UiCommand::ShowWarning(
                "Wait for this agent to finish (or press Esc) before starting a new session"
                    .to_string(),
            ))
            .await;
        return false;
    };

    let session_tx = Transaction::session_start(agent_id);
    let store = session.kernel.store();

    if let Err(e) = store.enqueue_tx(&session_tx) {
        error!(error = %e, "Failed to enqueue session start");
    } else if let Ok(Some((token, tx))) = store.dequeue_tx(agent_id) {
        match session.kernel.process_dequeued(tx.clone(), token).await {
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

    session.messages.clear();

    let _ = state
        .commands
        .send(UiCommand::SetStatus("Ready".to_string()))
        .await;
    true
}

/// A system line for the chat.
pub(super) fn system_message(content: String) -> UiCommand {
    UiCommand::ShowMessage(MessageData {
        role: MessageRole::System,
        content,
        is_streaming: false,
        timestamp_ms: None,
        command: None,
        activity: None,
        model: None,
        usage: None,
    })
}

pub(super) async fn show_system_message(state: &LoopState<'_>, content: String) {
    let _ = state.commands.send(system_message(content)).await;
}

/// Probe the local model server, publish the model list, and explain how
/// to get the local default running when it isn't.
pub(super) async fn announce_models(state: &mut LoopState<'_>) {
    state.models.refresh_local().await;
    state.auth.refresh_plan_tiers().await;
    let summary = super::auth::auth_summary(
        &state.auth,
        state.agent_loop.config_mut().auth_token.as_deref(),
    );
    state.models.set_auth(summary);
    if let Some(note) = state.models.refresh_plans(state.auth.as_ref()).await {
        show_system_message(state, note).await;
    }
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
