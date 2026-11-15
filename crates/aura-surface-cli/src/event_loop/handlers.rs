//! Handler functions for agent turns, auth, and session lifecycle.

use super::agent_events::ForwarderState;
use super::processes::{now_ms, turn_id, warn_kill_failed};
use super::record_ui::create_response_transaction;
use super::{forward_agent_events, LoopState, TURN_TIMEOUT};
use aura_agent::AgentLoopEvent;
use aura_core_types::Transaction;
use aura_terminal::events::{ProcessKind, ProcessStatus, ProcessSummary};
use aura_terminal::{UiCommand, UiEvent};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Run one turn of the active agent on `agent_loop` with `tools`.
///
/// UI events keep flowing while the turn runs: Esc cancels it, quit
/// cancels it and ends the session, and anything else waits in
/// `state.deferred` until the turn is over. The returned flag is set
/// when the user cancelled the turn.
pub(super) async fn run_agent_turn(
    state: &mut LoopState<'_>,
    agent_loop: &aura_agent::AgentLoop,
    tools: Vec<aura_model_reasoner::ToolDefinition>,
    model: &str,
    recorder: Option<std::sync::Arc<aura_agent_trace::RunRecorder>>,
) -> (
    Result<aura_agent::AgentLoopResult, aura_agent::AgentError>,
    ForwarderState,
    bool,
) {
    let (agent_event_tx, agent_event_rx) = tokio::sync::mpsc::channel::<AgentLoopEvent>(1024);

    let fwd_commands = state.commands.clone();
    let forwarder = tokio::spawn(forward_agent_events(agent_event_rx, fwd_commands, recorder));

    let cancel_token = CancellationToken::new();
    let cancel_for_turn = cancel_token.clone();
    let mut cancelled = false;
    let row = turn_id(state.session.agent_id());
    state
        .processes
        .start(turn_summary(state, model), Some(cancel_token.clone()));

    let turn = tokio::time::timeout(
        TURN_TIMEOUT,
        agent_loop.run_with_events(
            &state.session.model_gateway,
            &state.session.tool_gateway,
            state.session.messages.clone(),
            tools,
            Some(agent_event_tx),
            Some(cancel_token),
        ),
    );
    tokio::pin!(turn);
    let process_result = loop {
        tokio::select! {
            outcome = &mut turn => break match outcome {
                Ok(result) => result,
                Err(_) => {
                    cancel_for_turn.cancel();
                    Err(aura_agent::AgentError::Timeout(format!(
                        "Agent turn timed out after {} minutes",
                        TURN_TIMEOUT.as_secs() / 60
                    )))
                }
            },
            Some(event) = state.events.recv() => match event {
                UiEvent::Cancel => {
                    info!("Cancelling the running turn");
                    cancel_for_turn.cancel();
                    cancelled = true;
                }
                UiEvent::Quit => {
                    info!("Quit during a turn; cancelling it");
                    cancel_for_turn.cancel();
                    cancelled = true;
                    state.quit_requested = true;
                }
                UiEvent::KillProcess(id) => {
                    handle_kill_process(state, &id).await;
                    cancelled |= cancel_for_turn.is_cancelled();
                }
                UiEvent::OpenGraph(id) => super::trace::open_graph(state, id).await,
                UiEvent::ListRuns => super::trace::list(state).await,
                UiEvent::LabelStep { run_id, seq, value } => {
                    super::trace::label(state, &run_id, seq, value).await;
                }
                UiEvent::ShowStatus => super::trace::status_report(state).await,
                other => state.deferred.push_back(other),
            },
        }
    };
    let status = match &process_result {
        _ if cancelled => ProcessStatus::Killed,
        Ok(result) if result.llm_error.is_none() => ProcessStatus::Exited,
        _ => ProcessStatus::Failed,
    };
    state.processes.finish(&row, status);

    let forwarded = match forwarder.await {
        Ok(fwd_state) => {
            if fwd_state.thinking_active {
                let _ = state.commands.send(UiCommand::FinishThinking).await;
            }
            if fwd_state.streaming_active {
                let _ = state.commands.send(UiCommand::FinishStreaming).await;
            }
            fwd_state
        }
        Err(e) => {
            warn!(error = %e, "Event forwarder panicked");
            ForwarderState::default()
        }
    };

    (process_result, forwarded, cancelled)
}

pub(super) async fn handle_agent_success(
    state: &mut LoopState<'_>,
    result: aura_agent::AgentLoopResult,
    forwarded: &ForwarderState,
) {
    state.session.messages = result.messages.clone();

    persist_response_via_kernel(state, &result.total_text).await;
    emit_response_to_ui(state, &result, forwarded).await;

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

    match state.session.kernel.process_direct(response_tx).await {
        Ok(result) => {
            debug!(
                seq = result.entry.seq,
                "Response record persisted via kernel"
            );
        }
        Err(e) => {
            error!(error = %e, "Failed to persist response record via kernel");
        }
    }
}

async fn emit_response_to_ui(
    state: &LoopState<'_>,
    result: &aura_agent::AgentLoopResult,
    forwarded: &ForwarderState,
) {
    if !result.total_text.is_empty() {
        let preview: String = result.total_text.chars().take(100).collect();
        info!(response_preview = %preview, "Model response received");

        if !forwarded.had_text {
            let _ = state
                .commands
                .send(UiCommand::ShowMessage(aura_terminal::events::MessageData {
                    role: aura_terminal::events::MessageRole::Assistant,
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

pub(super) async fn handle_new_session(state: &mut LoopState<'_>) {
    debug!("New session requested");
    let agent_id = state.session.agent_id();
    super::trace::close(state, agent_id);

    let session_tx = Transaction::session_start(state.session.agent_id());
    let store = state.session.kernel.store();

    if let Err(e) = store.enqueue_tx(&session_tx) {
        error!(error = %e, "Failed to enqueue session start");
    } else if let Ok(Some((token, tx))) = store.dequeue_tx(state.session.agent_id()) {
        match state.session.kernel.process_dequeued(tx, token).await {
            Ok(result) => {
                debug!(
                    seq = result.entry.seq,
                    "Session start record persisted via kernel"
                );
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
            model: None,
            usage: None,
        }))
        .await;
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

/// Processes tab row for the active agent's turn on `model`.
fn turn_summary(state: &LoopState<'_>, model: &str) -> ProcessSummary {
    let prompt = state
        .session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == aura_model_reasoner::Role::User)
        .map(aura_model_reasoner::Message::text_content)
        .unwrap_or_default();
    let agent = state.session.agent_id().to_hex();
    ProcessSummary {
        id: turn_id(state.session.agent_id()),
        kind: ProcessKind::Turn,
        label: prompt.lines().next().unwrap_or_default().to_string(),
        detail: prompt,
        status: ProcessStatus::Running,
        owner_name: state.session.identity.name.clone(),
        jump_agent_id: agent.clone(),
        owner_agent_id: agent,
        started_ms: now_ms(),
        model: Some(model.to_string()),
        killable: true,
        ..ProcessSummary::default()
    }
}

/// Kill what the Processes tab row `id` runs.
pub(super) async fn handle_kill_process(state: &LoopState<'_>, id: &str) {
    let command = match state.processes.kill(id) {
        Ok(killed) => {
            info!(process = %id, kind = killed.kind.label(), owner = %killed.owner, "Killed from the Processes tab");
            UiCommand::ShowSuccess(format!("Stopped {}", killed.label))
        }
        Err(problem) => {
            warn_kill_failed(id, &problem);
            UiCommand::ShowWarning(problem)
        }
    };
    let _ = state.commands.send(command).await;
}
