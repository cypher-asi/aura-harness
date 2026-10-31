//! Per-agent sessions and agent switching for the terminal UI.
//!
//! Every registered agent gets its own kernel (and therefore its own
//! record) plus its own model conversation. Kernels are opened lazily the
//! first time an agent is selected and kept for the rest of the run, so
//! switching back resumes the in-memory conversation.

use super::LoopState;
use crate::record_loader::{agent_summaries, conversation_history, recent_record_summaries};
use aura_agent::{KernelModelGateway, KernelToolGateway};
use aura_agent_kernel::Kernel;
use aura_core_types::{AgentId, Identity};
use aura_model_reasoner::Message;
use aura_store_db::ReadStore;
use aura_terminal::UiCommand;
use std::sync::Arc;
use tracing::{info, warn};

/// Live state for one agent.
pub struct AgentSession {
    pub identity: Identity,
    pub kernel: Arc<Kernel>,
    pub model_gateway: KernelModelGateway,
    pub tool_gateway: KernelToolGateway,
    /// Model conversation for this agent's current session.
    pub messages: Vec<Message>,
}

impl AgentSession {
    #[must_use]
    pub const fn agent_id(&self) -> AgentId {
        self.identity.agent_id
    }
}

/// Constructs the kernel for one agent.
pub type KernelBuilder = Box<dyn Fn(AgentId) -> anyhow::Result<Kernel> + Send + Sync>;

/// Opens per-agent sessions and reads agent records for the UI.
pub struct KernelFactory {
    store: Arc<dyn ReadStore>,
    build_kernel: KernelBuilder,
}

impl KernelFactory {
    #[must_use]
    pub fn new(store: Arc<dyn ReadStore>, build_kernel: KernelBuilder) -> Self {
        Self {
            store,
            build_kernel,
        }
    }

    /// Open a session (kernel + gateways) for `identity`.
    ///
    /// # Errors
    /// Returns an error if the kernel cannot be constructed (e.g. the
    /// store read or record replay fails).
    pub fn open(&self, identity: Identity) -> anyhow::Result<AgentSession> {
        let kernel = Arc::new((self.build_kernel)(identity.agent_id)?);
        Ok(AgentSession {
            identity,
            model_gateway: KernelModelGateway::new(kernel.clone()),
            tool_gateway: KernelToolGateway::new(kernel.clone()),
            kernel,
            messages: Vec::new(),
        })
    }
}

pub(super) async fn handle_select_agent(state: &mut LoopState<'_>, agent_hex: &str) {
    match AgentId::from_hex(agent_hex) {
        Ok(agent_id) => activate_agent(state, agent_id).await,
        Err(e) => {
            warn!(agent = agent_hex, error = %e, "Invalid agent id from UI");
            let _ = state
                .commands
                .send(UiCommand::ShowError(format!("Invalid agent id: {e}")))
                .await;
        }
    }
}

pub(super) async fn handle_create_agent(state: &mut LoopState<'_>, name: Option<String>) {
    match state.registry.create(name) {
        Ok(agent) => {
            info!(zns_id = %agent.zns_id, name = %agent.name, "Created terminal agent");
            activate_agent(state, agent.agent_id()).await;
            let _ = state
                .commands
                .send(UiCommand::ShowSuccess(format!(
                    "Created agent {}",
                    agent.name
                )))
                .await;
        }
        Err(e) => {
            let _ = state
                .commands
                .send(UiCommand::ShowError(format!("Could not create agent: {e}")))
                .await;
        }
    }
}

pub(super) async fn handle_refresh_agents(state: &LoopState<'_>) {
    send_agent_list(state).await;
}

async fn activate_agent(state: &mut LoopState<'_>, agent_id: AgentId) {
    if agent_id == state.session.agent_id() {
        send_agent_list(state).await;
        return;
    }
    let Some(registered) = state.registry.find(agent_id).cloned() else {
        let _ = state
            .commands
            .send(UiCommand::ShowError("Unknown agent".to_string()))
            .await;
        return;
    };

    let next = match state.parked.remove(&agent_id) {
        Some(session) => session,
        None => match state.factory.open(registered.identity()) {
            Ok(session) => session,
            Err(e) => {
                warn!(error = %e, "Failed to open agent kernel");
                let _ = state
                    .commands
                    .send(UiCommand::ShowError(format!(
                        "Could not open agent {}: {e}",
                        registered.name
                    )))
                    .await;
                return;
            }
        },
    };
    let previous = std::mem::replace(&mut state.session, next);
    state.parked.insert(previous.agent_id(), previous);
    super::install_memory_observer(state);

    if let Err(e) = state.registry.set_active(agent_id) {
        warn!(error = %e, "Failed to persist active agent");
    }

    show_active_agent(state).await;
    let _ = state
        .commands
        .send(UiCommand::SetStatus(format!(
            "Switched to {}",
            registered.name
        )))
        .await;
}

/// Replace the Record panel and chat with the active agent's history.
pub(super) async fn show_active_agent(state: &LoopState<'_>) {
    let store = state.factory.store.as_ref();
    let agent_id = state.session.agent_id();

    let _ = state.commands.send(UiCommand::ClearRecords).await;
    match recent_record_summaries(store, agent_id) {
        Ok(summaries) => {
            for summary in summaries {
                let _ = state.commands.send(UiCommand::NewRecord(summary)).await;
            }
        }
        Err(e) => {
            let _ = state
                .commands
                .send(UiCommand::ShowWarning(format!(
                    "Could not load record history: {e}"
                )))
                .await;
        }
    }

    let history = conversation_history(store, agent_id).unwrap_or_else(|e| {
        warn!(error = %e, "Failed to rebuild conversation from record");
        Vec::new()
    });
    let _ = state
        .commands
        .send(UiCommand::LoadConversation(history))
        .await;

    send_sandbox_root(state).await;
    send_agent_list(state).await;
}

/// Point the Sandbox panel at the active agent's workspace, creating it
/// so the panel has something to show before the first tool call.
pub(super) async fn send_sandbox_root(state: &LoopState<'_>) {
    let workspace = state.session.kernel.agent_workspace();
    if let Err(e) = tokio::fs::create_dir_all(&workspace).await {
        warn!(path = %workspace.display(), error = %e, "Failed to create agent workspace");
    }
    let _ = state
        .commands
        .send(UiCommand::SetSandboxRoot(workspace))
        .await;
}

pub(super) async fn send_agent_list(state: &LoopState<'_>) {
    let active = state.session.agent_id();
    let summaries = agent_summaries(&state.registry, state.factory.store.as_ref(), active);
    let _ = state.commands.send(UiCommand::SetAgents(summaries)).await;
    let _ = state
        .commands
        .send(UiCommand::SetActiveAgent(active.to_hex()))
        .await;
}
