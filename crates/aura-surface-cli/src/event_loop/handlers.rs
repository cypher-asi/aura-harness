//! Handler functions for agent turns, auth, and session lifecycle.

use super::agent_events::{AgentUi, CostCap, ForwarderState, TurnPlan};
use super::agents::AgentSession;
use super::hud::{context_view, send_session, todo_views};
use super::processes::{now_ms, warn_kill_failed};
use super::record_ui::create_response_transaction;
use super::steering::Steering;
use super::turns::{record_user_prompt, Prompt, Work};
use super::{forward_agent_events, LoopState};
use crate::long_run::LongRunSettings;
use crate::project_instructions::agents_md_in;
use crate::record_loader::chat_from_messages;
use crate::session_checkpoint::SessionListing;
use aura_agent::{AgentLoopEvent, RunOptions, WithPostEditCheck};
use aura_context_prompts::model_messages::init_project::init_project_prompt;
use aura_core_types::AgentId;
use aura_core_types::{RecordEntry, Transaction};
use aura_terminal::events::{MessageData, MessageRole, ProcessKind, ProcessStatus, ProcessSummary};
use aura_terminal::UiCommand;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Run one turn of `session` on `agent_loop` with `tools`, streaming it
/// into `ui`. `cancel` stops it early, `steering` feeds it messages while
/// it runs, and `limits` may stop it for time or cost. `recorder` traces
/// the turn's events into its conversation's run.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_agent_turn(
    session: &AgentSession,
    agent_loop: &aura_agent::AgentLoop,
    tools: Vec<aura_model_reasoner::ToolDefinition>,
    ui: &AgentUi,
    cancel: &CancellationToken,
    steering: &Steering,
    limits: LongRunSettings,
    recorder: Option<std::sync::Arc<aura_agent_trace::RunRecorder>>,
) -> (
    Result<aura_agent::AgentLoopResult, aura_agent::AgentError>,
    ForwarderState,
) {
    let (agent_event_tx, agent_event_rx) = tokio::sync::mpsc::channel::<AgentLoopEvent>(1024);
    // A timeout or cost stop ends the turn without it counting as
    // cancelled by the user.
    let turn_cancel = cancel.child_token();
    let cost_cap = limits.max_turn_cost_usd.map(|limit_usd| CostCap {
        limit_usd,
        stop: turn_cancel.clone(),
    });
    let forwarder = tokio::spawn(forward_agent_events(
        agent_event_rx,
        ui.clone(),
        cost_cap,
        agent_loop.config().max_context_tokens,
        TurnPlan {
            workspace: session.kernel.agent_workspace(),
            building: session
                .building_plan
                .then(|| session.active_plan.clone())
                .flatten(),
        },
        recorder,
    ));

    let process_result = drive_turn(
        session,
        agent_loop,
        tools,
        agent_event_tx,
        &turn_cancel,
        steering,
        limits.turn_timeout,
    )
    .await
    .unwrap_or_else(|| {
        let minutes = limits.turn_timeout.map_or(0, |limit| limit.as_secs() / 60);
        Err(aura_agent::AgentError::Timeout(format!(
            "Agent turn timed out after {minutes} minutes (AURA_TUI_TURN_TIMEOUT)"
        )))
    });

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

/// Run one turn of `session` on `agent_loop` with `tools`, sending its
/// events to `events`. `turn_cancel` stops it early and `steering` feeds
/// it messages while it runs. A session with a post-edit check runs it
/// after each step that edits files. Returns `None`, after cancelling
/// `turn_cancel`, when it runs longer than `timeout`.
pub(crate) async fn drive_turn(
    session: &AgentSession,
    agent_loop: &aura_agent::AgentLoop,
    tools: Vec<aura_model_reasoner::ToolDefinition>,
    events: tokio::sync::mpsc::Sender<AgentLoopEvent>,
    turn_cancel: &CancellationToken,
    steering: &Steering,
    timeout: Option<std::time::Duration>,
) -> Option<Result<aura_agent::AgentLoopResult, aura_agent::AgentError>> {
    let executor = WithPostEditCheck::new(&session.tool_gateway, session.post_edit_check.as_ref());
    let run = agent_loop.run_with_session(
        &session.model_gateway,
        &executor,
        session.messages.clone(),
        tools,
        RunOptions {
            event_tx: Some(events),
            cancellation_token: Some(turn_cancel.clone()),
            handle: Some(steering.handle()),
        },
    );
    match timeout {
        None => Some(run.await),
        Some(limit) => {
            let result = tokio::time::timeout(limit, run).await.ok();
            if result.is_none() {
                turn_cancel.cancel();
            }
            result
        }
    }
}

/// Keep what the turn produced: its conversation, todo list and reply.
/// `steered` are the messages the model got while the turn ran; they are
/// recorded as prompts ahead of the reply.
pub(super) async fn handle_agent_success(
    session: &mut AgentSession,
    result: aura_agent::AgentLoopResult,
    forwarded: &ForwarderState,
    ui: &AgentUi,
    steered: &[String],
) {
    session.messages.clone_from(&result.messages);
    session.todos.clone_from(&result.todos);
    ui.send(UiCommand::SetContext(context_view(
        &result,
        forwarded.context_limit,
    )))
    .await;
    ui.send(UiCommand::SetTodos(todo_views(&session.todos)))
        .await;

    for text in steered {
        if let Err(problem) = record_user_prompt(session, text.clone()).await {
            warn!(%problem, "Failed to record a mid-turn message");
        }
    }
    // The kernels' record observer shows the entry in the Record panel.
    record_response(session, &result.total_text).await;
    session.save_checkpoint().await;
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

/// Record the model's reply `total_text` on `session`. A failure is
/// logged; the checkpoint still holds the reply.
pub(crate) async fn record_response(
    session: &AgentSession,
    total_text: &str,
) -> Option<(Transaction, RecordEntry)> {
    let response_tx = create_response_transaction(session.agent_id(), total_text);

    match session.kernel.process_direct(response_tx.clone()).await {
        Ok(result) => {
            debug!(
                seq = result.entry.seq,
                "Response record persisted via kernel"
            );
            Some((response_tx, result.entry))
        }
        Err(e) => {
            error!(error = %e, "Failed to persist response record via kernel");
            None
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

    mark_session_start(session).await;
    session.reset();
    super::trace::close(state, agent_id);
    let ui =
        AgentUi::new(state.commands.clone(), agent_id).with_shown(state.hub.record_panel.clone());
    ui.send(UiCommand::SetTodos(Vec::new())).await;
    ui.send_if_shown(UiCommand::SetPlan(None)).await;

    let _ = state
        .commands
        .send(UiCommand::SetStatus("Ready".to_string()))
        .await;
    true
}

/// Mark a new session in `session`'s record, returning the transaction and
/// its record entry. A failure is logged.
pub(crate) async fn mark_session_start(
    session: &AgentSession,
) -> Option<(Transaction, RecordEntry)> {
    let agent_id = session.agent_id();
    let session_tx = Transaction::session_start(agent_id);
    let store = session.kernel.store();

    if let Err(e) = store.enqueue_tx(&session_tx) {
        error!(error = %e, "Failed to enqueue session start");
        return None;
    }
    let Ok(Some((token, tx))) = store.dequeue_tx(agent_id) else {
        return None;
    };
    match session.kernel.process_dequeued(tx.clone(), token).await {
        Ok(result) => {
            debug!(
                seq = result.entry.seq,
                "Session start record persisted via kernel"
            );
            Some((tx, result.entry))
        }
        Err(e) => {
            error!(error = %e, "Failed to persist session start record via kernel");
            None
        }
    }
}

/// Make the saved session `checkpoint` the current session of `session`:
/// mark the switch in the record, so a rebuild from it starts here, then
/// continue from the checkpoint and save it as current.
pub(crate) async fn resume_checkpoint(
    session: &mut AgentSession,
    checkpoint: crate::session_checkpoint::SessionCheckpoint,
) -> Option<(Transaction, RecordEntry)> {
    let marked = mark_session_start(session).await;
    session.restore(checkpoint);
    session.save_checkpoint().await;
    marked
}

/// `/resume`: list the earlier sessions of `agent_id`, or continue the one
/// at 1-based `index` in that list.
pub(super) async fn handle_resume(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    index: Option<usize>,
) {
    let ui = AgentUi::new(state.commands.clone(), agent_id);
    let Some(session) = state.idle.get_mut(&agent_id) else {
        ui.send(UiCommand::ShowWarning(
            "Wait for this agent to finish before resuming a session".to_string(),
        ))
        .await;
        return;
    };
    let Some(checkpoints) = session.checkpoints.clone() else {
        ui.send(UiCommand::ShowWarning(
            "Sessions are not saved in this terminal".to_string(),
        ))
        .await;
        return;
    };
    let sessions = checkpoints.list(agent_id, Some(session.started_ms));
    let Some(index) = index else {
        ui.send(system_message(session_list(&sessions))).await;
        return;
    };
    let Some(listing) = index.checked_sub(1).and_then(|i| sessions.get(i)) else {
        ui.send(UiCommand::ShowWarning(format!(
            "No session {index}; /resume lists them"
        )))
        .await;
        return;
    };
    let Some(checkpoint) = checkpoints.load(agent_id, listing.started_ms) else {
        ui.send(UiCommand::ShowWarning(
            "Could not read that session".to_string(),
        ))
        .await;
        return;
    };

    info!(%agent_id, started_ms = listing.started_ms, "Resuming an earlier session");
    resume_checkpoint(session, checkpoint).await;
    ui.send(UiCommand::LoadConversation(chat_from_messages(
        &session.messages,
    )))
    .await;
    send_session(&ui, session).await;
    ui.send(UiCommand::ShowSuccess(format!(
        "Resumed: {}",
        listing.title
    )))
    .await;
}

/// `/init`: ask `agent_id` to study its project and write the `AGENTS.md`
/// at its root, or improve the one already there. It runs as a normal
/// turn, queued behind any work the agent is doing.
pub(super) async fn handle_init_project(state: &mut LoopState<'_>, agent_id: AgentId) {
    let workspace = match state.kernel_for(agent_id) {
        Some(kernel) => kernel.agent_workspace(),
        None => match super::agents::open_session(state, agent_id) {
            Ok(session) => {
                let workspace = session.kernel.agent_workspace();
                state.idle.insert(agent_id, session);
                workspace
            }
            Err(problem) => {
                let ui = AgentUi::new(state.commands.clone(), agent_id);
                ui.send(UiCommand::ShowError(problem)).await;
                ui.send(UiCommand::Complete).await;
                return;
            }
        },
    };
    let existing = agents_md_in(&workspace).map(|path| path.display().to_string());
    info!(
        %agent_id,
        workspace = %workspace.display(),
        existing = existing.is_some(),
        "Asking the agent to write AGENTS.md"
    );
    let text = init_project_prompt(&workspace.display().to_string(), existing.as_deref());
    let work = Work::Prompt {
        prompt: Prompt::plain(text),
        shown: false,
    };
    super::submit(state, agent_id, work).await;
}

/// The `/resume` list.
fn session_list(sessions: &[SessionListing]) -> String {
    if sessions.is_empty() {
        return "No earlier sessions for this agent".to_string();
    }
    let mut out = String::from("Earlier sessions (resume one with /resume <n>):");
    for (i, session) in sessions.iter().enumerate() {
        out.push_str(&format!(
            "\n  {}. {} · {} messages · {}",
            i + 1,
            format_session_time(session.updated_ms),
            session.messages,
            session.title
        ));
    }
    out
}

/// `Nov 02 14:03` in local time.
fn format_session_time(ts_ms: u64) -> String {
    i64::try_from(ts_ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map_or_else(
            || "?".to_string(),
            |dt| {
                dt.with_timezone(&chrono::Local)
                    .format("%b %d %H:%M")
                    .to_string()
            },
        )
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
    state.models.refresh_route(state.agent_loop.config_mut());
    let _ = state.commands.send(state.models.ui_command()).await;
    if let Some(hint) = state.models.local_hint() {
        show_system_message(state, hint).await;
    }
}

pub(super) async fn handle_select_model(state: &mut LoopState<'_>, id: &str) {
    use aura_model_reasoner::model_catalog::{backend_for, ModelBackend, GATE_MODEL_ID};
    if id == GATE_MODEL_ID || backend_for(id) == ModelBackend::Local {
        state.models.refresh_local().await;
    }
    match state.models.select(state.agent_loop.config_mut(), id) {
        Ok(()) => {
            info!(model = %state.models.current(), "Model selected");
            let _ = state
                .commands
                .send(UiCommand::ShowSuccess(format!(
                    "Model: {}",
                    state.models.display()
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

/// Processes tab row for a turn of `agent_id` on `model`, answering
/// `prompt` (or doing `prompt`, for work that isn't a model reply).
pub(super) fn turn_summary(agent_id: AgentId, prompt: &str, model: &str) -> ProcessSummary {
    let agent = agent_id.to_hex();
    ProcessSummary {
        id: super::processes::turn_id(agent_id),
        kind: ProcessKind::Turn,
        label: prompt.lines().next().unwrap_or_default().to_string(),
        detail: prompt.to_string(),
        status: ProcessStatus::Running,
        jump_agent_id: agent.clone(),
        owner_agent_id: agent,
        started_ms: now_ms(),
        model: Some(model.to_string()),
        killable: true,
        ..ProcessSummary::default()
    }
}

/// Kill what the Processes tab row `id` runs. A turn or task waiting for
/// an approval is denied, so it sees the cancellation.
pub(super) async fn handle_kill_process(state: &LoopState<'_>, id: &str) {
    let command = match state.processes.kill(id) {
        Ok(killed) => {
            info!(process = %id, kind = killed.kind.label(), owner = %killed.owner, "Killed from the Processes tab");
            let agent = id
                .strip_prefix("turn:")
                .or_else(|| id.strip_prefix("task:"))
                .and_then(|hex| AgentId::from_hex(hex).ok());
            if let Some(agent) = agent {
                state.approvals.cancel_agent(agent).await;
            }
            UiCommand::ShowSuccess(format!("Stopped {}", killed.label))
        }
        Err(problem) => {
            warn_kill_failed(id, &problem);
            UiCommand::ShowWarning(problem)
        }
    };
    let _ = state.commands.send(command).await;
}
