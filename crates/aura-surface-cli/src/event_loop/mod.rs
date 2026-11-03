//! Event processing loop for the terminal UI mode.
//!
//! Agents work in parallel: each turn runs on its own task (see
//! [`turns`]), so the loop keeps handling the UI while agents work, and the
//! user can switch to another agent and give it work too. Each agent does
//! one thing at a time; work sent to a busy agent waits for it.

pub(crate) mod agent_events;
pub(crate) mod agents;
pub(crate) mod auth;
pub(crate) mod handlers;
pub(crate) mod record_ui;
pub(crate) mod run_command;
pub(crate) mod subagents;
mod turns;

pub(crate) use agent_events::forward_agent_events;
pub use agents::{AgentSession, KernelBuilder, KernelFactory};
pub(crate) use record_ui::send_record_to_ui;
pub use record_ui::UiToolAuditObserver;
pub use subagents::{AgentHub, AgentHubParts, TurnBase};

use crate::model_selection::ModelSelection;
use agent_events::AgentUi;
use aura_agent::{AgentLoop, ProcessManager};
use aura_agent_kernel::Kernel;
use aura_auth::{AuthManager, PendingLink};
use aura_core_types::{AgentId, Transaction};
use aura_model_reasoner::model_catalog::local_model_name;
use aura_model_reasoner::ToolDefinition;
use aura_terminal::{UiCommand, UiEvent};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use turns::{Task, Work, WorkDone};

const TURN_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How long quitting waits for cancelled turns to record how they ended.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

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
    pub(super) events: &'a mut mpsc::Receiver<UiEvent>,
    pub(super) agent_loop: &'a mut AgentLoop,
    pub(super) memory_manager: Option<Arc<aura_context_memory::MemoryManager>>,
    pub(super) models: ModelSelection,
    /// The agent the UI is showing.
    pub(super) active: AgentId,
    /// Sessions of the opened agents with no work in flight. An agent at
    /// work has its session on the task doing the work.
    pub(super) idle: HashMap<AgentId, AgentSession>,
    /// Work waiting for a busy agent, oldest first
    queued: HashMap<AgentId, VecDeque<Work>>,
    /// Where agent work reports that it ended
    work_done: mpsc::Sender<WorkDone>,
    /// Work started by the loop that hasn't ended yet
    in_flight: usize,
    pub(super) hub: Arc<AgentHub>,
    pub(super) auth: Arc<AuthManager>,
    /// Plan sign-in waiting for a pasted code.
    pub(super) pending_link: Option<PendingLink>,
}

impl LoopState<'_> {
    /// Kernel of `agent_id`, whether it is idle or at work.
    pub(super) fn kernel_for(&self, agent_id: AgentId) -> Option<Arc<Kernel>> {
        self.idle
            .get(&agent_id)
            .map(|s| Arc::clone(&s.kernel))
            .or_else(|| self.hub.running_kernel(agent_id))
    }

    /// Keep the sessions of finished spawns so switching to them resumes
    /// their conversation on the kernel that wrote it, then start any work
    /// that was waiting for them.
    async fn park_finished_spawns(&mut self) {
        for session in self.hub.take_finished() {
            let agent_id = session.agent_id();
            self.idle.insert(agent_id, session);
            start_next(self, agent_id).await;
        }
    }

    /// Publish the terminal's current config as the base for spawned agents.
    fn sync_turn_base(&mut self) {
        let config = self.agent_loop.config_mut().clone();
        let mut base = subagents::lock(&self.hub.base);
        base.config = config;
        base.limits = self.models.limits();
    }

    /// The agent named by hex id `agent_hex`, or the active agent when the
    /// UI didn't name a valid one.
    fn agent_or_active(&self, agent_hex: &str) -> AgentId {
        AgentId::from_hex(agent_hex).unwrap_or(self.active)
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
    let (done_tx, mut done_rx) = mpsc::channel::<WorkDone>(64);

    let active = session.agent_id();
    let mut state = LoopState {
        commands: &commands,
        events,
        agent_loop,
        memory_manager,
        models,
        active,
        idle: HashMap::from([(active, session)]),
        queued: HashMap::new(),
        work_done: done_tx,
        in_flight: 0,
        hub,
        auth,
        pending_link: None,
    };

    agents::send_sandbox_root(&state).await;
    handlers::announce_models(&mut state).await;

    let hub = Arc::clone(&state.hub);
    loop {
        tokio::select! {
            Some(done) = done_rx.recv() => {
                handle_work_done(&mut state, done).await;
            }
            () = hub.spawn_finished.notified() => {
                state.park_finished_spawns().await;
                agents::send_agent_list(&state).await;
            }
            Some(completion_tx) = process_completions.recv() => {
                handle_completion(&mut state, completion_tx).await;
            }
            Some(outcome) = link_rx.recv() => {
                auth::handle_link_outcome(&mut state, outcome).await;
            }
            event = state.events.recv() => {
                let Some(event) = event else { break };
                if handle_ui_event(&mut state, event, &link_tx).await {
                    break;
                }
            }
        }
    }

    shut_down(&mut state, &mut done_rx).await;
    Ok(())
}

/// Cancel every agent's work and give it a moment to record how it ended.
async fn shut_down(state: &mut LoopState<'_>, done_rx: &mut mpsc::Receiver<WorkDone>) {
    state.queued.clear();
    state.hub.cancel_all();
    let deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE;
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
}

async fn handle_completion(state: &mut LoopState<'_>, completion_tx: Transaction) {
    info!(
        hash = %completion_tx.hash,
        tx_type = ?completion_tx.tx_type,
        "Processing async process completion"
    );

    let agent_id = completion_tx.agent_id;
    let Some(kernel) = state.kernel_for(agent_id) else {
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
                if agent_id == state.active {
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
        UiEvent::UserMessage {
            agent_id,
            text,
            shown,
        } => {
            let agent_id = state.agent_or_active(&agent_id);
            submit(state, agent_id, Work::Prompt { text, shown }).await;
        }
        UiEvent::RunCommand {
            agent_id,
            command,
            cols,
        } => {
            let agent_id = state.agent_or_active(&agent_id);
            submit(state, agent_id, Work::Command { command, cols }).await;
        }
        UiEvent::Approve(_id) => debug!("Approval received"),
        UiEvent::Deny(_id) => debug!("Denial received"),
        UiEvent::Quit => {
            debug!("Quit received");
            return true;
        }
        UiEvent::Cancel(agent_id) => {
            let agent_id = state.agent_or_active(&agent_id);
            debug!(%agent_id, "Cancel received");
            if !state.hub.cancel(agent_id) {
                AgentUi::new(state.commands.clone(), agent_id)
                    .send(UiCommand::SetStatus("Cancelled".to_string()))
                    .await;
            }
        }
        UiEvent::ShowStatus | UiEvent::ShowHelp | UiEvent::ShowHistory(_) => {}
        UiEvent::Clear => {
            if handlers::handle_new_session(state).await {
                let _ = state.commands.send(UiCommand::ClearConversation).await;
            }
        }
        UiEvent::NewSession => {
            handlers::handle_new_session(state).await;
        }
        UiEvent::SelectAgent(agent_hex) => agents::handle_select_agent(state, &agent_hex).await,
        UiEvent::CreateAgent(name) => agents::handle_create_agent(state, name).await,
        UiEvent::CreateProject { name, folder } => {
            agents::handle_create_project(state, &name, &folder).await;
        }
        UiEvent::RenameProject(name) => agents::handle_rename_project(state, &name).await,
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

/// Give `work` to `agent_id`: now if it is idle, else once its current
/// work ends.
async fn submit(state: &mut LoopState<'_>, agent_id: AgentId, work: Work) {
    if state.hub.is_running(agent_id) {
        info!(%agent_id, "Agent is busy; queueing work");
        state.queued.entry(agent_id).or_default().push_back(work);
        return;
    }
    start_work(state, agent_id, work).await;
}

/// Start the oldest work waiting for `agent_id`, if any.
async fn start_next(state: &mut LoopState<'_>, agent_id: AgentId) {
    let Some(queue) = state.queued.get_mut(&agent_id) else {
        return;
    };
    let next = queue.pop_front();
    if queue.is_empty() {
        state.queued.remove(&agent_id);
    }
    if let Some(work) = next {
        start_work(state, agent_id, work).await;
    }
}

/// Run `work` on `agent_id` in the background.
async fn start_work(state: &mut LoopState<'_>, agent_id: AgentId, work: Work) {
    let ui = AgentUi::new(state.commands.clone(), agent_id);
    let session = match state.idle.remove(&agent_id) {
        Some(session) => session,
        None => match agents::open_session(state, agent_id) {
            Ok(session) => session,
            Err(problem) => {
                ui.send(UiCommand::ShowError(problem)).await;
                ui.send(UiCommand::Complete).await;
                return;
            }
        },
    };

    let task = match work {
        Work::Prompt { text, shown } => {
            info!(%agent_id, text = %text, "Processing user message");
            // A local model with no server used to sit there until the
            // 10-minute request timeout; the probe answers in well under a
            // second.
            if local_model_unavailable(state, &ui).await {
                state.idle.insert(agent_id, session);
                return;
            }
            ui.send(UiCommand::TurnStarted {
                prompt: (!shown).then(|| text.clone()),
            })
            .await;
            let (agent_loop, tools) = prepare_turn(state, &session).await;
            Task::Turn {
                text,
                agent_loop: Box::new(agent_loop),
                tools,
            }
        }
        Work::Command { command, cols } => Task::Command { command, cols },
    };

    let cancel = CancellationToken::new();
    state
        .hub
        .start_running(agent_id, Arc::clone(&session.kernel), cancel.clone());
    state.in_flight += 1;
    turns::spawn(session, task, ui, cancel, state.work_done.clone());
    agents::send_agent_list(state).await;
}

/// The agent loop and tools for a turn of `session`.
async fn prepare_turn(
    state: &mut LoopState<'_>,
    session: &AgentSession,
) -> (AgentLoop, Vec<ToolDefinition>) {
    for problem in subagents::lock(&state.hub.definitions).reload() {
        warn!(%problem, "Skipping agent definition");
    }
    state.hub.send_definitions().await;
    state.sync_turn_base();
    let (mut config, tools) = state.hub.turn(session.definition.as_deref());
    if let Some(ref mm) = state.memory_manager {
        mm.prepare_context(session.agent_id(), &mut config.system_prompt)
            .await;
        config.observers.clear();
        config
            .observers
            .push(aura_engine::memory_observer::MemoryTurnObserver::new(
                Arc::clone(mm),
                session.agent_id(),
                None,
                Vec::new(),
                None,
            ));
    }
    (AgentLoop::new(config), tools)
}

/// Take back the session of work that ended, report a failure, and start
/// what was waiting for the agent.
async fn handle_work_done(state: &mut LoopState<'_>, done: WorkDone) {
    let WorkDone {
        agent_id,
        session,
        failure,
    } = done;
    state.in_flight = state.in_flight.saturating_sub(1);
    if let Some(session) = session {
        state.idle.insert(agent_id, session);
    }
    state.hub.stop_running(agent_id);

    if let Some(problem) = failure {
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

/// Re-probe Ollama when the selected model is local. Returns `true` after
/// telling `ui` why it can't run and ending its turn.
async fn local_model_unavailable(state: &mut LoopState<'_>, ui: &AgentUi) -> bool {
    if local_model_name(state.models.current()).is_none() {
        return false;
    }
    state.models.refresh_local().await;
    let _ = state.commands.send(state.models.ui_command()).await;
    let Some(hint) = state.models.local_hint() else {
        return false;
    };
    info!("Skipping local turn; model server is not ready");
    ui.send(handlers::system_message(hint)).await;
    ui.send(UiCommand::Complete).await;
    true
}

#[cfg(test)]
mod tests;
