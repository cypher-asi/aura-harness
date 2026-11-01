//! Event processing loop for the terminal UI mode.

pub(crate) mod agent_events;
pub(crate) mod agents;
pub(crate) mod auth;
pub(crate) mod handlers;
pub(crate) mod record_ui;
pub(crate) mod run_command;
pub(crate) mod subagents;

pub(crate) use agent_events::forward_agent_events;
pub use agents::{AgentSession, KernelBuilder, KernelFactory};
pub(crate) use record_ui::send_record_to_ui;
pub use record_ui::UiToolAuditObserver;
pub use subagents::{AgentHub, AgentHubParts, TurnBase};

use crate::model_selection::ModelSelection;
use aura_agent::{AgentLoop, ProcessManager};
use aura_agent_kernel::Kernel;
use aura_auth::{AuthManager, PendingLink};
use aura_core_types::{AgentId, Transaction};
use aura_model_reasoner::Message;
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
    /// Session for the agent shown when the loop starts.
    pub session: AgentSession,
    /// Agents, permanent agents, and the kernel factory, shared with spawns.
    pub hub: Arc<AgentHub>,
    pub _process_manager: Arc<ProcessManager>,
    /// Optional memory manager for prompt injection and result ingestion.
    pub memory_manager: Option<Arc<aura_context_memory::MemoryManager>>,
    /// Model selection already applied to `agent_loop`.
    pub models: ModelSelection,
    /// Sign-in state shared with the model provider.
    pub auth: Arc<AuthManager>,
}

/// Mutable state threaded through all event handlers.
pub(super) struct LoopState<'a> {
    pub(super) commands: &'a mpsc::Sender<UiCommand>,
    pub(super) agent_loop: &'a mut AgentLoop,
    pub(super) memory_manager: Option<Arc<aura_context_memory::MemoryManager>>,
    pub(super) models: ModelSelection,
    /// The agent the UI is showing; user messages go here.
    pub(super) session: AgentSession,
    /// Previously opened agents, kept so their conversations survive switching.
    pub(super) parked: HashMap<AgentId, AgentSession>,
    pub(super) hub: Arc<AgentHub>,
    pub(super) auth: Arc<AuthManager>,
    /// Plan sign-in waiting for a pasted code.
    pub(super) pending_link: Option<PendingLink>,
}

impl LoopState<'_> {
    fn kernel_for(&self, agent_id: AgentId) -> Option<&Arc<Kernel>> {
        if agent_id == self.session.agent_id() {
            Some(&self.session.kernel)
        } else {
            self.parked.get(&agent_id).map(|s| &s.kernel)
        }
    }

    /// Keep the sessions of finished spawns so switching to them resumes
    /// their conversation on the kernel that wrote it.
    fn park_finished_spawns(&mut self) {
        for session in self.hub.take_finished() {
            self.parked.insert(session.agent_id(), session);
        }
    }

    /// Publish the terminal's current config as the base for spawned agents.
    fn sync_turn_base(&mut self) {
        let config = self.agent_loop.config_mut().clone();
        let mut base = subagents::lock(&self.hub.base);
        base.config = config;
        base.limits = self.models.limits();
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
        session,
        hub,
        _process_manager,
        memory_manager,
        models,
        auth,
    } = ctx;
    let (link_tx, mut link_rx) = mpsc::channel::<auth::LinkOutcome>(4);

    let mut state = LoopState {
        commands: &commands,
        agent_loop,
        memory_manager,
        models,
        session,
        parked: HashMap::new(),
        hub,
        auth,
        pending_link: None,
    };
    install_memory_observer(&mut state);

    agents::send_sandbox_root(&state).await;
    handlers::announce_models(&mut state).await;

    loop {
        tokio::select! {
            Some(completion_tx) = process_completions.recv() => {
                handle_completion(&mut state, completion_tx).await;
            }
            Some(outcome) = link_rx.recv() => {
                auth::handle_link_outcome(&mut state, outcome).await;
            }
            Some(event) = events.recv() => {
                if handle_ui_event(&mut state, event, &link_tx).await {
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
async fn handle_ui_event(
    state: &mut LoopState<'_>,
    event: UiEvent,
    link_outcomes: &mpsc::Sender<auth::LinkOutcome>,
) -> bool {
    match event {
        UiEvent::UserMessage(text) => {
            handle_user_message(state, text).await;
            agents::send_agent_list(state).await;
        }
        UiEvent::RunCommand { command, cols } => {
            run_command::handle_run_command(state, &command, cols).await;
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
        UiEvent::InstantiateDefinition(name) => {
            agents::handle_instantiate_definition(state, &name).await;
        }
        UiEvent::EditDefinition(name) => agents::handle_edit_definition(state, name).await,
        UiEvent::DeleteDefinition(name) => agents::handle_delete_definition(state, &name).await,
        UiEvent::DefinitionEdited(path) => agents::handle_definition_edited(state, &path).await,
        UiEvent::LoginCredentials { email, password } => {
            auth::handle_login(state, &email, &password).await;
        }
        UiEvent::Logout => auth::handle_logout(state).await,
        UiEvent::Whoami => auth::handle_whoami(state).await,
        UiEvent::LinkSubscription { provider, paste } => {
            auth::handle_link(state, &provider, paste, link_outcomes).await;
        }
        UiEvent::LinkCode(code) => auth::handle_link_code(state, &code).await,
        UiEvent::Unlink(provider) => auth::handle_unlink(state, &provider).await,
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

    let prompt_tx = Transaction::user_prompt(state.session.agent_id(), text.clone());
    let (tx, token) = match enqueue_and_dequeue(state, prompt_tx).await {
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

    for problem in subagents::lock(&state.hub.definitions).reload() {
        warn!(%problem, "Skipping agent definition");
    }
    state.hub.send_definitions().await;
    state.sync_turn_base();
    let (config, tools) = state.hub.turn(state.session.definition.as_deref());
    let turn_loop = AgentLoop::new(config);
    let (process_result, streamed_text) = handlers::run_agent_turn(state, &turn_loop, tools).await;
    state.park_finished_spawns();

    match process_result {
        Ok(result) => {
            handlers::handle_agent_success(state, result, streamed_text).await;
        }
        Err(e) => {
            error!(error = %e, "Agent loop failed");
            let _ = state
                .commands
                .send(UiCommand::ShowError(e.to_string()))
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
    tx: Transaction,
) -> Option<(Transaction, aura_store_db::DequeueToken)> {
    let store = state.session.kernel.store();
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
