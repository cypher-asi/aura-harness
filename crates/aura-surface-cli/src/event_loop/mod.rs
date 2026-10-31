//! Event processing loop for the terminal UI mode.

pub(crate) mod agent_events;
pub(crate) mod agents;
pub(crate) mod handlers;
pub(crate) mod record_ui;

pub(crate) use agent_events::forward_agent_events;
pub use agents::{AgentSession, KernelFactory};
pub(crate) use record_ui::send_record_to_ui;

use crate::agent_registry::AgentRegistry;
use crate::model_selection::ModelSelection;
use aura_agent::{AgentLoop, ProcessManager};
use aura_agent_kernel::Kernel;
use aura_core_types::{AgentId, Transaction};
use aura_model_reasoner::{Message, ToolDefinition};
use aura_terminal::{UiCommand, UiEvent};
use std::collections::HashMap;
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
    pub tools: &'a [ToolDefinition],
    /// Session for the agent shown when the loop starts.
    pub session: AgentSession,
    /// Builds kernels for agents selected later in the run.
    pub factory: KernelFactory,
    pub registry: AgentRegistry,
    pub _process_manager: Arc<ProcessManager>,
    /// Optional memory manager for prompt injection and result ingestion.
    pub memory_manager: Option<Arc<aura_context_memory::MemoryManager>>,
    /// Model selection already applied to `agent_loop`.
    pub models: ModelSelection,
}

/// Mutable state threaded through all event handlers.
pub(super) struct LoopState<'a> {
    pub(super) commands: &'a mpsc::Sender<UiCommand>,
    pub(super) agent_loop: &'a mut AgentLoop,
    pub(super) tools: &'a [ToolDefinition],
    pub(super) memory_manager: Option<Arc<aura_context_memory::MemoryManager>>,
    pub(super) models: ModelSelection,
    /// The agent the UI is showing; user messages go here.
    pub(super) session: AgentSession,
    /// Previously opened agents, kept so their conversations survive switching.
    pub(super) parked: HashMap<AgentId, AgentSession>,
    pub(super) factory: KernelFactory,
    pub(super) registry: AgentRegistry,
}

impl LoopState<'_> {
    fn kernel_for(&self, agent_id: AgentId) -> Option<&Arc<Kernel>> {
        if agent_id == self.session.agent_id() {
            Some(&self.session.kernel)
        } else {
            self.parked.get(&agent_id).map(|s| &s.kernel)
        }
    }
}

/// Point the memory observer at the active agent. The TUI installs no
/// other observers, so the list is rebuilt from scratch.
fn install_memory_observer(state: &mut LoopState<'_>) {
    if let Some(ref mm) = state.memory_manager {
        let observers = &mut state.agent_loop.config_mut().observers;
        observers.clear();
        observers.push(aura_engine::memory_observer::MemoryTurnObserver::new(
            Arc::clone(mm),
            state.session.agent_id(),
            None,
            Vec::new(),
            None,
        ));
    }
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
        tools,
        session,
        factory,
        registry,
        _process_manager,
        memory_manager,
        models,
    } = ctx;

    let mut state = LoopState {
        commands: &commands,
        agent_loop,
        tools,
        memory_manager,
        models,
        session,
        parked: HashMap::new(),
        factory,
        registry,
    };
    install_memory_observer(&mut state);

    handlers::announce_models(&mut state).await;

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

    let agent_id = completion_tx.agent_id;
    let Some(kernel) = state.kernel_for(agent_id).cloned() else {
        warn!(%agent_id, "Process completion for an agent with no open kernel");
        return;
    };
    let store = kernel.store();

    if let Err(e) = store.enqueue_tx(&completion_tx) {
        error!(error = %e, "Failed to enqueue completion transaction");
        return;
    }

    if let Ok(Some((token, tx))) = store.dequeue_tx(agent_id) {
        match kernel.process_dequeued(tx.clone(), token).await {
            Ok(result) => {
                debug!(
                    seq = result.entry.seq,
                    "Completion record persisted via kernel"
                );
                if agent_id == state.session.agent_id() {
                    send_record_to_ui(state.commands, result.entry.seq, &tx, &result.entry).await;
                    let _ = state
                        .commands
                        .send(UiCommand::SetStatus("Process completed".to_string()))
                        .await;
                } else {
                    agents::send_agent_list(state).await;
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to persist completion record via kernel");
            }
        }
    }
}

/// Returns `true` if the loop should break (quit).
async fn handle_ui_event(state: &mut LoopState<'_>, event: UiEvent) -> bool {
    match event {
        UiEvent::UserMessage(text) => {
            handle_user_message(state, text).await;
            agents::send_agent_list(state).await;
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
            handlers::handle_new_session(state).await;
            let _ = state.commands.send(UiCommand::ClearConversation).await;
        }
        UiEvent::NewSession => handlers::handle_new_session(state).await,
        UiEvent::SelectAgent(agent_hex) => agents::handle_select_agent(state, &agent_hex).await,
        UiEvent::CreateAgent(name) => agents::handle_create_agent(state, name).await,
        UiEvent::RefreshAgents => agents::handle_refresh_agents(state).await,
        UiEvent::LoginCredentials { email, password } => {
            handlers::handle_login(state, &email, &password).await;
        }
        UiEvent::Logout => handlers::handle_logout(state).await,
        UiEvent::Whoami => handlers::handle_whoami(state).await,
        UiEvent::SelectModel(id) => handlers::handle_select_model(state, &id).await,
    }
    false
}

async fn handle_user_message(state: &mut LoopState<'_>, text: String) {
    info!(text = %text, "Processing user message");

    let _ = state
        .commands
        .send(UiCommand::SetStatus("Thinking...".to_string()))
        .await;

    drain_stale_inbox(state).await;

    let (tx, token) = match enqueue_and_dequeue(state, &text).await {
        Some(v) => v,
        None => return,
    };

    let prompt_result = match state
        .session
        .kernel
        .process_dequeued(tx.clone(), token)
        .await
    {
        Ok(result) => result,
        Err(e) => {
            error!(error = %e, "Failed to persist prompt record via kernel");
            let _ = state
                .commands
                .send(UiCommand::ShowError(format!("Kernel error: {e}")))
                .await;
            let _ = state.commands.send(UiCommand::Complete).await;
            return;
        }
    };
    send_record_to_ui(
        state.commands,
        prompt_result.entry.seq,
        &tx,
        &prompt_result.entry,
    )
    .await;

    state.session.messages.push(Message::user(text));

    if let Some(ref mm) = state.memory_manager {
        mm.prepare_context(
            state.session.agent_id(),
            &mut state.agent_loop.config_mut().system_prompt,
        )
        .await;
    }

    let (process_result, streamed_text) = handlers::run_agent_turn(state).await;

    match process_result {
        Ok(result) => {
            handlers::handle_agent_success(state, result, streamed_text).await;
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
    let store = state.session.kernel.store();
    let mut stale_count = 0;
    while let Ok(Some((token, tx))) = store.dequeue_tx(state.session.agent_id()) {
        warn!(
            stale_inbox_seq = token.inbox_seq(),
            stale_tx_type = ?tx.tx_type,
            "Discarding stale inbox transaction"
        );
        match state.session.kernel.process_dequeued(tx, token).await {
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

async fn enqueue_and_dequeue(
    state: &mut LoopState<'_>,
    text: &str,
) -> Option<(Transaction, aura_store_db::DequeueToken)> {
    let store = state.session.kernel.store();
    let tx = Transaction::user_prompt(state.session.agent_id(), text.to_string());
    if let Err(e) = store.enqueue_tx(&tx) {
        error!(error = %e, "Failed to enqueue transaction");
        let _ = state
            .commands
            .send(UiCommand::ShowError(format!("Storage error: {e}")))
            .await;
        let _ = state.commands.send(UiCommand::Complete).await;
        return None;
    }

    let (token, dequeued_tx) = match store.dequeue_tx(state.session.agent_id()) {
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

    Some((tx, token))
}
