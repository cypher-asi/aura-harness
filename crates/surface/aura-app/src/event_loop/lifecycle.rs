//! Work and background commands ending, and the loop shutting down.

use super::agent_events::AgentUi;
use super::delivery::{prompt_work, start_next};
use super::start_work::local_model_unavailable;
use super::turns::WorkDone;
use super::{agents, channels, inbox, notices, processes, LoopState, SHUTDOWN_GRACE};
use aura_core_types::{AgentId, Transaction};
use aura_ui::events::ProcessStatus;
use aura_ui::UiCommand;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Cancel every agent's work and give it a moment to record how it ended.
pub(super) async fn shut_down(state: &mut LoopState<'_>, done_rx: &mut mpsc::Receiver<WorkDone>) {
    info!(
        in_flight = state.in_flight,
        "Shutting down; cancelling agent work"
    );
    let started = tokio::time::Instant::now();
    state.queued.clear();
    state.approvals.cancel_all();
    state.hub.cancel_all();
    let deadline = started + SHUTDOWN_GRACE;
    while state.in_flight > 0 {
        match tokio::time::timeout_at(deadline, done_rx.recv()).await {
            Ok(Some(_)) => state.in_flight -= 1,
            Ok(None) | Err(_) => {
                warn!(
                    in_flight = state.in_flight,
                    "Quitting before every agent stopped"
                );
                break;
            }
        }
    }
    info!(
        in_flight = state.in_flight,
        elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "Agent work shut down"
    );
}

/// A background command ended: record it for the agent that started it,
/// and tell the agent.
pub(super) async fn handle_completion(state: &mut LoopState<'_>, completion_tx: Transaction) {
    info!(
        hash = %completion_tx.hash,
        tx_type = ?completion_tx.tx_type,
        "Processing async process completion"
    );
    let notice = notices::completion_notice(&state.processes, &completion_tx);
    record_completion(state, completion_tx).await;
    if let Some(notice) = notice {
        notices::handle_notice(state, notice).await;
    }
}

/// Write a process completion to the record of the agent that started it.
pub(super) async fn record_completion(state: &mut LoopState<'_>, completion_tx: Transaction) {
    let agent_id = completion_tx.agent_id;
    let Some(kernel) = state.kernel_for(agent_id) else {
        warn!(%agent_id, "Process completion for an agent with no open kernel");
        return;
    };
    // The kernels' record observer shows the entry.
    if notices::record_completion(&kernel, &completion_tx).await {
        if agent_id == state.active {
            let _ = state
                .commands
                .send(UiCommand::SetStatus("Process completed".to_string()))
                .await;
        } else {
            agents::send_agent_list(state).await;
        }
    }
}

/// Take back the session of work that ended, report a failure, and start
/// what was waiting for the agent: first any messages sent into the turn
/// too late for the model to see, then the queue.
pub(super) async fn handle_work_done(state: &mut LoopState<'_>, done: WorkDone) {
    let WorkDone {
        agent_id,
        session,
        failure,
        unseen,
        refused_models,
        health,
    } = done;
    if !refused_models.is_empty() {
        for model in &refused_models {
            state.models.drop_model(model);
        }
        let _ = state.commands.send(state.models.ui_command()).await;
    }
    if !unseen.is_empty() {
        for (source, text) in unseen.into_iter().rev() {
            // A user message sent mid-turn is already in the chat.
            state
                .queued
                .push_front(agent_id, prompt_work(source, text, true));
        }
    }
    state.in_flight = state.in_flight.saturating_sub(1);
    let parked = session.is_some();
    if let Some(session) = session {
        state.idle.insert(agent_id, session);
    }
    let held = finish_turn(state, agent_id, failure.as_deref(), &health).await;
    if parked {
        super::idle_sessions::left_idle(state, agent_id).await;
    }
    if health.model_worked {
        super::recover::model_worked(state, agent_id).await;
    }

    // A held turn's chat says why it waits instead.
    if let Some(problem) = failure.filter(|_| !held) {
        let ui = AgentUi::new(state.commands.clone(), agent_id);
        if !local_model_unavailable(state, &ui).await {
            ui.send(UiCommand::ShowError(problem)).await;
            ui.send(UiCommand::Complete).await;
        }
    }

    state.park_finished_spawns().await;
    start_next(state, agent_id).await;
    agents::send_agent_list(state).await;
}

/// Whether `agent_id`'s turn, which went as `health` says, is held for the
/// recovery: its trouble one a pause or a retry may get past. A held turn
/// ends failed in the Processes tab and settles nothing.
async fn held(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    health: &super::recover::TurnHealth,
) -> bool {
    let Some(trouble) = &health.trouble else {
        return false;
    };
    if !super::recover::hold_turn(state, agent_id, trouble).await {
        return false;
    }
    state
        .processes
        .finish(&processes::turn_id(agent_id), ProcessStatus::Failed);
    true
}

/// Mark the turn of `agent_id`, which ended with `failure` if any, ended
/// in the Processes tab, and settle its channel replies and what it owed
/// through its inbox. A turn the user stopped to carry on leaves what it
/// owes through its inbox for the next turn that ends well. A turn whose
/// trouble a pause or a retry may get past (`health`) is held instead,
/// all it owes kept; returns whether it was.
async fn finish_turn(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    failure: Option<&str>,
    health: &super::recover::TurnHealth,
) -> bool {
    let cancelled = state.hub.stop_running(agent_id);
    let interrupted = state.hub.inbox.take_interrupted(agent_id) && cancelled;
    if !cancelled && held(state, agent_id, health).await {
        return true;
    }
    // A model error no pause or retry got past fails a turn that owes a
    // report: its sender hears why, not what the turn wrote before it.
    let owes = state.hub.inbox.owed_by(agent_id) > 0;
    let errored = health.llm_error.as_deref().filter(|_| owes && !cancelled);
    let failure = failure.or(errored);
    let failed = failure.is_some();
    if let Some(why) = failure {
        state
            .hub
            .status
            .fail(agent_id, &format!("Its turn failed: {why}"));
    }
    let status = if failed {
        ProcessStatus::Failed
    } else if cancelled {
        ProcessStatus::Killed
    } else {
        ProcessStatus::Exited
    };
    state
        .processes
        .finish(&processes::turn_id(agent_id), status);
    let ok = !failed && !cancelled;
    // A plan turn's answer waits for the build the factory starts next.
    let how = super::factory::hooks::TurnState {
        ok,
        interrupted: interrupted && !failed,
    };
    let waits = super::factory::hooks::after_local_turn(state, agent_id, how).await;
    if ok {
        channels::post_replies(state, agent_id).await;
        if !waits {
            inbox::answer(state, agent_id);
        }
    } else {
        channels::drop_unanswered(state, agent_id);
        if interrupted && !failed {
            info!(%agent_id, "The user stopped the turn; its inbox answers wait for the next one");
            state.hub.inbox.park(agent_id);
        } else {
            inbox::give_up(state, agent_id, failure.unwrap_or("it was stopped"));
            inbox::give_up_if_archived(&state.hub, agent_id);
        }
    }
    false
}
