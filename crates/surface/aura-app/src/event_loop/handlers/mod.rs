//! Handler functions for agent turns, auth, and session lifecycle.
//!
//! Turn handling lives here; session lifecycle, model selection and
//! Processes tab handlers live in the sibling modules and are re-exported
//! so their paths stay `event_loop::handlers::*`.

mod models;
mod process_rows;
mod session;

pub(in crate::event_loop) use models::{
    announce_models, handle_select_model, handle_set_thinking, show_system_message, system_message,
};
pub(in crate::event_loop) use process_rows::{handle_kill_process, turn_summary};
pub(in crate::event_loop) use session::{handle_init_project, handle_new_session, handle_resume};
pub(crate) use session::{mark_session_start, resume_checkpoint};

use super::agent_events::{AgentUi, CostCap, ForwarderState, TurnPlan};
use super::agents::AgentSession;
use super::forward_agent_events;
use super::hud::{context_view, todo_views};
use super::record_ui::create_response_transaction;
use super::steering::Steering;
use super::turns::record_user_prompt;
use crate::long_run::LongRunSettings;
use aura_agent::{AgentLoopEvent, AgentToolExecutor, RunOptions, WithPostEditCheck};
use aura_core_types::{RecordEntry, Transaction};
use aura_gate::lead::LeadExecutor;
use aura_ui::events::{MessageData, MessageRole};
use aura_ui::UiCommand;
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
    tools: Vec<aura_model::ToolDefinition>,
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
        turn_plan(session),
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
    .unwrap_or_else(|| Err(turn_timeout_error(limits.turn_timeout)));

    let forwarded = finish_forwarder(forwarder, ui).await;
    (process_result, forwarded)
}

/// What the event forwarder needs to follow `session`'s plan and tasks.
fn turn_plan(session: &AgentSession) -> TurnPlan {
    TurnPlan {
        agent: session.agent_id(),
        tasks: std::sync::Arc::clone(&session.tasks),
        workspace: session.kernel.agent_workspace(),
        building: session
            .building_plan
            .then(|| session.active_plan.clone())
            .flatten(),
    }
}

/// The error for a turn stopped after running longer than `turn_timeout`.
fn turn_timeout_error(turn_timeout: Option<std::time::Duration>) -> aura_agent::AgentError {
    let minutes = turn_timeout.map_or(0, |limit| limit.as_secs() / 60);
    aura_agent::AgentError::Timeout(format!(
        "Agent turn timed out after {minutes} minutes (AURA_TUI_TURN_TIMEOUT)"
    ))
}

/// Wait for the event `forwarder` to finish, closing any thinking or
/// streaming it left open in `ui`.
async fn finish_forwarder(
    forwarder: tokio::task::JoinHandle<ForwarderState>,
    ui: &AgentUi,
) -> ForwarderState {
    match forwarder.await {
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
    }
}

/// Run one turn of `session` on `agent_loop` with `tools`, sending its
/// events to `events`. `turn_cancel` stops it early and `steering` feeds
/// it messages while it runs. A session with a post-edit check runs it
/// after each step that edits files, and a lead's calls stay within the
/// lead's limits. Returns `None`, after cancelling `turn_cancel`, when it
/// runs longer than `timeout`.
pub(crate) async fn drive_turn(
    session: &AgentSession,
    agent_loop: &aura_agent::AgentLoop,
    tools: Vec<aura_model::ToolDefinition>,
    events: tokio::sync::mpsc::Sender<AgentLoopEvent>,
    turn_cancel: &CancellationToken,
    steering: &Steering,
    timeout: Option<std::time::Duration>,
) -> Option<Result<aura_agent::AgentLoopResult, aura_agent::AgentError>> {
    let lead = LeadExecutor::new(&session.tool_gateway);
    let tool_gateway: &dyn AgentToolExecutor = if session.lead {
        &lead
    } else {
        &session.tool_gateway
    };
    let executor = WithPostEditCheck::new(tool_gateway, session.post_edit_check.as_ref());
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
    let finished = super::plan_mode::finish_done_todos(session);
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
    // Shown before it is recorded: the kernels' record observer shows the
    // entry in the Record panel and the turn's models under the reply.
    emit_response_to_ui(&result, forwarded, ui).await;
    record_response(session, &result.total_text).await;
    if let Some(plan) = &finished {
        let workspace = session.kernel.agent_workspace();
        super::plan_mode::announce_finished_plan(ui, &workspace, plan).await;
    }
    session.save_checkpoint().await;

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
                model: None,
                usage: None,
                sources: Vec::new(),
                boot: None,
            }))
            .await;
        }
    }
}
