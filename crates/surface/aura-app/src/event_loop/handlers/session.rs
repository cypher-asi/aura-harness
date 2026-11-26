//! Session lifecycle handlers: new session, `/resume` and `/init`.

use super::system_message;
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::agents::AgentSession;
use crate::event_loop::hud::send_session;
use crate::event_loop::turns::{Prompt, Work};
use crate::event_loop::LoopState;
use crate::project_instructions::agents_md_in;
use crate::record_loader::chat_from_messages;
use crate::session_checkpoint::{Checkpoints, SessionCheckpoint, SessionListing};
use aura_context_prompts::model_messages::init_project::init_project_prompt;
use aura_core_types::AgentId;
use aura_core_types::{RecordEntry, Transaction};
use aura_ui::UiCommand;
use tracing::{debug, error, info};

/// Start a new session on the active agent. Returns `false`, after
/// saying why, while the agent is busy.
pub(in crate::event_loop) async fn handle_new_session(state: &mut LoopState<'_>) -> bool {
    debug!("New session requested");
    let agent_id = state.active;
    if state.cloud.node_writes(agent_id) {
        if !crate::event_loop::cloud::new_session(state, agent_id).await {
            return false;
        }
        if let Some(session) = state.idle.get_mut(&agent_id) {
            session.reset();
        }
    } else {
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
    }
    state.hub.traces.close(agent_id);
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
    checkpoint: SessionCheckpoint,
) -> Option<(Transaction, RecordEntry)> {
    let marked = mark_session_start(session).await;
    session.restore(checkpoint);
    session.save_checkpoint().await;
    marked
}

/// `/resume`: list the earlier sessions of `agent_id`, or continue the one
/// at 1-based `index` in that list.
pub(in crate::event_loop) async fn handle_resume(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    index: Option<usize>,
) {
    let ui = AgentUi::new(state.commands.clone(), agent_id);
    if state.cloud.node_writes(agent_id) {
        ui.send(UiCommand::ShowWarning(
            "Earlier sessions resume on this machine: bring the agent back (/local) first"
                .to_string(),
        ))
        .await;
        return;
    }
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
    let (listing, checkpoint) = match pick_session(&checkpoints, agent_id, &sessions, index) {
        Ok(picked) => picked,
        Err(warning) => {
            ui.send(UiCommand::ShowWarning(warning)).await;
            return;
        }
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

/// The session at 1-based `index` in `sessions` and its checkpoint, or
/// the warning to show when there is none or it can't be read.
fn pick_session<'a>(
    checkpoints: &Checkpoints,
    agent_id: AgentId,
    sessions: &'a [SessionListing],
    index: usize,
) -> Result<(&'a SessionListing, SessionCheckpoint), String> {
    let listing = index
        .checked_sub(1)
        .and_then(|i| sessions.get(i))
        .ok_or_else(|| format!("No session {index}; /resume lists them"))?;
    let checkpoint = checkpoints
        .load(agent_id, listing.started_ms)
        .ok_or_else(|| "Could not read that session".to_string())?;
    Ok((listing, checkpoint))
}

/// `/init`: ask `agent_id` to study its project and write the `AGENTS.md`
/// at its root, or improve the one already there. It runs as a normal
/// turn, queued behind any work the agent is doing.
pub(in crate::event_loop) async fn handle_init_project(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
) {
    let workspace = match state.kernel_for(agent_id) {
        Some(kernel) => kernel.agent_workspace(),
        None => match crate::event_loop::agents::open_session(state, agent_id) {
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
    crate::event_loop::submit(state, agent_id, work).await;
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
