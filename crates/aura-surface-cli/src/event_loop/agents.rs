//! Per-agent sessions, agent switching, and permanent agent management for
//! the terminal UI.
//!
//! Every registered agent gets its own kernel (and therefore its own
//! record) plus its own model conversation. Kernels are opened lazily the
//! first time an agent is selected and kept for the rest of the run, so
//! switching back resumes the in-memory conversation.

use super::subagents::lock;
use super::LoopState;
use crate::agent_registry::RegisteredAgent;
use crate::record_loader::{conversation_history, recent_record_summaries};
use aura_agent::{KernelModelGateway, KernelToolGateway};
use aura_agent_kernel::Kernel;
use aura_core_types::{AgentId, Identity};
use aura_model_reasoner::Message;
use aura_store_db::ReadStore;
use aura_terminal::UiCommand;
use std::path::Path;
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
    /// Permanent agent this is an instance of; its turns follow its rules.
    pub definition: Option<String>,
}

impl AgentSession {
    #[must_use]
    pub const fn agent_id(&self) -> AgentId {
        self.identity.agent_id
    }
}

/// Constructs the kernel for one agent, given the agents that spawned it
/// (nearest first).
pub type KernelBuilder = Box<dyn Fn(AgentId, Vec<AgentId>) -> anyhow::Result<Kernel> + Send + Sync>;

/// Opens per-agent sessions and reads agent records for the UI.
pub struct KernelFactory {
    pub(super) store: Arc<dyn ReadStore>,
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

    /// Open a session (kernel + gateways) for `agent`.
    ///
    /// # Errors
    /// Returns an error if the kernel cannot be constructed (e.g. the
    /// store read or record replay fails).
    pub fn open(
        &self,
        agent: &RegisteredAgent,
        parent_chain: Vec<AgentId>,
    ) -> anyhow::Result<AgentSession> {
        let identity = agent.identity();
        let kernel = Arc::new((self.build_kernel)(identity.agent_id, parent_chain)?);
        Ok(AgentSession {
            identity,
            model_gateway: KernelModelGateway::new(kernel.clone()),
            tool_gateway: KernelToolGateway::new(kernel.clone()),
            kernel,
            messages: Vec::new(),
            definition: agent.definition.clone(),
        })
    }
}

pub(super) async fn handle_select_agent(state: &mut LoopState<'_>, agent_hex: &str) {
    match AgentId::from_hex(agent_hex) {
        Ok(agent_id) => activate_agent(state, agent_id).await,
        Err(e) => {
            warn!(agent = agent_hex, error = %e, "Invalid agent id from UI");
            show_error(state, format!("Invalid agent id: {e}")).await;
        }
    }
}

pub(super) async fn handle_create_agent(state: &mut LoopState<'_>, name: Option<String>) {
    let created = lock(&state.hub.registry).create(name);
    match created {
        Ok(agent) => {
            info!(zns_id = %agent.zns_id, name = %agent.name, "Created terminal agent");
            activate_agent(state, agent.agent_id()).await;
            show_success(state, format!("Created agent {}", agent.name)).await;
        }
        Err(e) => show_error(state, format!("Could not create agent: {e}")).await,
    }
}

/// Start an instance of a permanent agent and switch to it.
pub(super) async fn handle_instantiate_definition(state: &mut LoopState<'_>, name: &str) {
    let definition = lock(&state.hub.definitions)
        .get(name)
        .map(|d| d.name.clone());
    let Some(definition) = definition else {
        show_error(state, format!("No permanent agent named {name}")).await;
        return;
    };
    let created = lock(&state.hub.registry).create_instance(&definition, None);
    match created {
        Ok(agent) => {
            info!(name = %agent.name, %definition, "Started permanent agent instance");
            activate_agent(state, agent.agent_id()).await;
            show_success(state, format!("Started {}", agent.name)).await;
        }
        Err(e) => show_error(state, format!("Could not start {definition}: {e}")).await,
    }
}

/// Open a permanent agent's file in the editor, creating a new one when
/// `name` is `None`.
pub(super) async fn handle_edit_definition(state: &LoopState<'_>, name: Option<String>) {
    let path = {
        let mut definitions = lock(&state.hub.definitions);
        match name {
            Some(name) => definitions
                .get(&name)
                .map(|d| d.path.clone())
                .ok_or_else(|| format!("No permanent agent named {name}")),
            None => definitions
                .create(None)
                .map_err(|e| format!("Could not create a permanent agent: {e:#}")),
        }
    };
    match path {
        Ok(path) => {
            state.hub.send_definitions().await;
            let _ = state.commands.send(UiCommand::OpenEditor(path)).await;
        }
        Err(problem) => show_error(state, problem).await,
    }
}

pub(super) async fn handle_delete_definition(state: &LoopState<'_>, name: &str) {
    let deleted = lock(&state.hub.definitions).delete(name);
    match deleted {
        Ok(definition) => {
            info!(name = %definition.name, "Deleted permanent agent");
            state.hub.send_definitions().await;
            show_success(
                state,
                format!("Deleted permanent agent {}", definition.name),
            )
            .await;
        }
        Err(e) => show_error(state, format!("{e:#}")).await,
    }
}

/// Reload the permanent agents after the editor closed on `path`.
pub(super) async fn handle_definition_edited(state: &LoopState<'_>, path: &Path) {
    let problems = state.hub.reload_definitions();
    state.hub.send_definitions().await;
    let file = path.display().to_string();
    if let Some(problem) = problems.iter().find(|p| p.starts_with(&file)) {
        let _ = state
            .commands
            .send(UiCommand::ShowWarning(format!("Not loaded: {problem}")))
            .await;
        return;
    }
    let name = lock(&state.hub.definitions)
        .all()
        .iter()
        .find(|d| d.path == path)
        .map(|d| d.name.clone());
    if let Some(name) = name {
        show_success(state, format!("Saved permanent agent {name}")).await;
    }
}

pub(super) async fn handle_refresh_agents(state: &LoopState<'_>) {
    state.hub.send_agent_list().await;
    state.hub.send_definitions().await;
}

async fn activate_agent(state: &mut LoopState<'_>, agent_id: AgentId) {
    if agent_id == state.session.agent_id() {
        state.hub.send_agent_list().await;
        return;
    }
    let registered = lock(&state.hub.registry).find(agent_id).cloned();
    let Some(registered) = registered else {
        show_error(state, "Unknown agent".to_string()).await;
        return;
    };

    let next = match state.parked.remove(&agent_id) {
        Some(session) => session,
        None => match state.hub.factory.open(&registered, Vec::new()) {
            Ok(session) => session,
            Err(e) => {
                warn!(error = %e, "Failed to open agent kernel");
                show_error(
                    state,
                    format!("Could not open agent {}: {e}", registered.name),
                )
                .await;
                return;
            }
        },
    };
    let previous = std::mem::replace(&mut state.session, next);
    state.parked.insert(previous.agent_id(), previous);
    super::install_memory_observer(state);

    if let Err(e) = lock(&state.hub.registry).set_active(agent_id) {
        warn!(error = %e, "Failed to persist active agent");
    }

    show_active_agent(state).await;
    let _ = state
        .commands
        .send(UiCommand::SetStatus("Ready".to_string()))
        .await;
}

/// Replace the Record panel and chat with the active agent's history.
pub(super) async fn show_active_agent(state: &LoopState<'_>) {
    let store = state.hub.factory.store.as_ref();
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
    state.hub.send_agent_list().await;
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
    state.hub.send_agent_list().await;
}

async fn show_error(state: &LoopState<'_>, message: String) {
    let _ = state.commands.send(UiCommand::ShowError(message)).await;
}

async fn show_success(state: &LoopState<'_>, message: String) {
    let _ = state.commands.send(UiCommand::ShowSuccess(message)).await;
}
