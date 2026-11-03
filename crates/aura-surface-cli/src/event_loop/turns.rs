//! Agent work running on its own task.
//!
//! Every turn (and every command typed in a chat) runs on a spawned task
//! that owns the agent's session until it is done, so agents work in
//! parallel and the event loop keeps handling the UI meanwhile. Each agent
//! still does one thing at a time: its record has a single writer.

use super::agent_events::AgentUi;
use super::agents::AgentSession;
use super::handlers::{handle_agent_success, run_agent_turn};
use super::record_ui::send_record_to_ui;
use super::run_command::run_user_command;
use aura_agent::AgentLoop;
use aura_core_types::{AgentId, Transaction};
use aura_model_reasoner::{Message, ToolDefinition};
use aura_terminal::UiCommand;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// Something the user asked an agent to do.
pub(super) enum Work {
    /// A message for the model. `shown` is whether the chat already shows
    /// it.
    Prompt { text: String, shown: bool },
    /// A shell command typed in the chat, run in a terminal `cols` wide.
    Command { command: String, cols: u16 },
}

/// Work ready to run.
pub(super) enum Task {
    /// A model turn answering `text`.
    Turn {
        text: String,
        agent_loop: Box<AgentLoop>,
        tools: Vec<ToolDefinition>,
    },
    /// A typed shell command.
    Command { command: String, cols: u16 },
}

/// An agent's work that ended, handing its session back.
pub(super) struct WorkDone {
    pub agent_id: AgentId,
    /// `None` when the task panicked; the session is reopened from the
    /// record the next time it is needed.
    pub session: Option<AgentSession>,
    /// Why a turn failed, for the event loop to report.
    pub failure: Option<String>,
}

/// Run `task` on `session` in the background, reporting to `ui`, and send
/// the session back on `done`.
pub(super) fn spawn(
    session: AgentSession,
    task: Task,
    ui: AgentUi,
    cancel: CancellationToken,
    done: mpsc::Sender<WorkDone>,
) {
    let agent_id = session.agent_id();
    tokio::spawn(async move {
        let work = tokio::spawn(run(session, task, ui, cancel));
        let (session, failure) = match work.await {
            Ok((session, failure)) => (Some(session), failure),
            Err(e) => {
                error!(%agent_id, error = %e, "Agent work panicked");
                (None, Some(format!("Agent stopped unexpectedly: {e}")))
            }
        };
        let _ = done
            .send(WorkDone {
                agent_id,
                session,
                failure,
            })
            .await;
    });
}

async fn run(
    mut session: AgentSession,
    task: Task,
    ui: AgentUi,
    cancel: CancellationToken,
) -> (AgentSession, Option<String>) {
    let failure = match task {
        Task::Turn {
            text,
            agent_loop,
            tools,
        } => run_prompt(&mut session, text, &agent_loop, tools, &ui, &cancel).await,
        Task::Command { command, cols } => {
            run_user_command(&mut session, &command, cols, &ui).await;
            None
        }
    };
    (session, failure)
}

/// Record `text` as the user's prompt and run a turn answering it.
/// Returns why the turn failed, if it did.
async fn run_prompt(
    session: &mut AgentSession,
    text: String,
    agent_loop: &AgentLoop,
    tools: Vec<ToolDefinition>,
    ui: &AgentUi,
    cancel: &CancellationToken,
) -> Option<String> {
    drain_stale_inbox(session).await;

    let prompt_tx = Transaction::user_prompt(session.agent_id(), text.clone());
    let (tx, token) = match enqueue_and_dequeue(session, prompt_tx) {
        Ok(v) => v,
        Err(problem) => {
            ui.send(UiCommand::ShowError(problem)).await;
            ui.send(UiCommand::Complete).await;
            return None;
        }
    };
    let prompt_result = match session.kernel.process_dequeued(tx.clone(), token).await {
        Ok(result) => result,
        Err(e) => {
            error!(error = %e, "Failed to persist prompt record via kernel");
            ui.send(UiCommand::ShowError(format!("Kernel error: {e}")))
                .await;
            ui.send(UiCommand::Complete).await;
            return None;
        }
    };
    send_record_to_ui(
        ui.commands(),
        prompt_result.entry.seq,
        &tx,
        &prompt_result.entry,
    )
    .await;

    session.messages.push(Message::user(text));

    let (process_result, forwarded) = run_agent_turn(session, agent_loop, tools, ui, cancel).await;
    let cancelled = cancel.is_cancelled();
    if cancelled {
        ui.send(UiCommand::ShowWarning("Turn cancelled".to_string()))
            .await;
    }

    match process_result {
        Ok(result) => {
            handle_agent_success(session, result, &forwarded, ui).await;
            None
        }
        Err(e) if cancelled => {
            info!(error = %e, "Cancelled turn ended with an error");
            ui.send(UiCommand::Complete).await;
            None
        }
        Err(e) => {
            error!(error = %e, "Agent loop failed");
            Some(e.to_string())
        }
    }
}

/// Clear transactions left in the agent's inbox by an earlier run.
pub(super) async fn drain_stale_inbox(session: &AgentSession) {
    let store = session.kernel.store();
    let mut stale_count = 0;
    while let Ok(Some((token, tx))) = store.dequeue_tx(session.agent_id()) {
        warn!(
            stale_inbox_seq = token.inbox_seq(),
            stale_tx_type = ?tx.tx_type,
            "Discarding stale inbox transaction"
        );
        match session.kernel.process_dequeued(tx, token).await {
            Ok(_result) => {
                stale_count += 1;
            }
            Err(e) => {
                error!(error = %e, "Failed to clear stale transaction via kernel");
                break;
            }
        }
        if stale_count > 10 {
            error!("Too many stale transactions, aborting drain");
            break;
        }
    }
}

/// Put `tx` through the agent's inbox, returning it with its dequeue token.
fn enqueue_and_dequeue(
    session: &AgentSession,
    tx: Transaction,
) -> Result<(Transaction, aura_store_db::DequeueToken), String> {
    let store = session.kernel.store();
    if let Err(e) = store.enqueue_tx(&tx) {
        error!(error = %e, "Failed to enqueue transaction");
        return Err(format!("Storage error: {e}"));
    }

    let (token, dequeued_tx) = match store.dequeue_tx(session.agent_id()) {
        Ok(Some(item)) => item,
        Ok(None) => {
            error!("Transaction was enqueued but not found in inbox");
            return Err("Storage error: the message was not queued".to_string());
        }
        Err(e) => {
            error!(error = %e, "Failed to dequeue transaction");
            return Err(format!("Storage error: {e}"));
        }
    };

    if dequeued_tx.hash != tx.hash {
        error!("Transaction mismatch after draining stale entries");
    }

    Ok((tx, token))
}
