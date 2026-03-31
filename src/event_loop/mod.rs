//! Event processing loop for the terminal UI mode.

mod agent_events;
mod handlers;
mod record_ui;

use agent_events::forward_agent_events;
use record_ui::{compute_context_hash, send_record_to_ui};

use aura_agent::{AgentLoop, KernelToolExecutor, ProcessManager};
use aura_core::{AgentId, RecordEntry, Transaction};
use aura_reasoner::{Message, ModelProvider, ToolDefinition};
use aura_store::{RocksStore, Store};
use aura_terminal::{UiCommand, UiEvent};
use std::sync::Arc;
use tokio::sync::mpsc;
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
pub(super) struct LoopState<'a> {
    pub(super) seq: u64,
    pub(super) messages: Vec<Message>,
    pub(super) commands: &'a mpsc::Sender<UiCommand>,
    pub(super) agent_loop: &'a mut AgentLoop,
    pub(super) provider: &'a dyn ModelProvider,
    pub(super) executor: &'a KernelToolExecutor,
    pub(super) tools: &'a [ToolDefinition],
    pub(super) store: Arc<RocksStore>,
    pub(super) agent_id: AgentId,
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

    if let Ok(Some((token, tx))) = state.store.dequeue_tx(state.agent_id) {
        let context_hash = compute_context_hash(state.seq, &tx);
        let entry = RecordEntry::builder(state.seq, tx.clone())
            .context_hash(context_hash)
            .build();

        if let Err(e) =
            state
                .store
                .append_entry_atomic(state.agent_id, state.seq, &entry, token.inbox_seq())
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
        UiEvent::NewSession => handlers::handle_new_session(state).await,
        UiEvent::SelectAgent(_) => debug!("Agent selection not yet implemented"),
        UiEvent::RefreshAgents => debug!("Agent refresh not yet implemented"),
        UiEvent::LoginCredentials { email, password } => {
            handlers::handle_login(state, &email, &password).await;
        }
        UiEvent::Logout => handlers::handle_logout(state).await,
        UiEvent::Whoami => handlers::handle_whoami(state).await,
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

    let (process_result, streamed_text) = handlers::run_agent_turn(state, &tx, inbox_seq).await;

    match process_result {
        Ok(result) => {
            handlers::handle_agent_success(state, result, &tx, inbox_seq, streamed_text).await;
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
    while let Ok(Some((stale_token, stale_tx))) = state.store.dequeue_tx(state.agent_id) {
        warn!(
            stale_inbox_seq = stale_token.inbox_seq(),
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
            stale_token.inbox_seq(),
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

    let (token, dequeued_tx) = match state.store.dequeue_tx(state.agent_id) {
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

    Some((tx, token.inbox_seq()))
}
