//! Background agents: `spawn_agent` and the tools that talk to them.
//!
//! The terminal's agents are roots in an [`AgentHost`], which runs the
//! agents they spawn on its own tasks. A child's reply to a terminal agent
//! comes back as a [`Notice`], so it starts a turn like any other message.

use super::command_output::TerminalCommandOutput;
use super::notices::{Notice, NoticeSender};
use async_trait::async_trait;
use aura_agent::AgentLoopConfig;
use aura_core_types::{AgentId, AgentMode, AgentPermissions};
use aura_engine::{
    AgentEnv, AgentHooks, AgentHost, AgentIdentity, ChildKernel, ChildKernelFactory,
    ChildKernelRequest, InboxMessage, ParentInbox, RootAgent, Scheduler,
};
use aura_exec_tools::agents::get_agent_state::GET_AGENT_STATE_TOOL_NAME;
use aura_exec_tools::agents::list_agents::LIST_AGENTS_TOOL_NAME;
use aura_exec_tools::agents::send_to_agent::SEND_TO_AGENT_TOOL_NAME;
use aura_exec_tools::agents::task::TASK_TOOL_NAME;
use aura_exec_tools::{BackgroundProcessHook, ToolCatalog, ToolConfig};
use aura_model::{ModelRequestKind, ToolDefinition};
use aura_ui::UiCommand;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Tools for working with background agents. `delegate_task` is left out:
/// for agents in this host it is `send_to_agent` under another name. `task`
/// is left out too: the terminal advertises its own `task` for permanent
/// agents, and tool names must be unique in a request.
#[must_use]
pub fn agent_tool_definitions() -> Vec<ToolDefinition> {
    aura_exec_tools::agents::cross_agent_catalog_entries()
        .into_iter()
        .map(|(_, definition, _)| definition)
        .filter(|definition| !matches!(definition.name.as_str(), "delegate_task" | TASK_TOOL_NAME))
        .collect()
}

/// The [`agent_tool_definitions`] agents talk to each other with. Every
/// top-level agent has them, background agents or not: they reach its
/// peers through their inboxes (see `inbox`), and the agents it spawned.
#[must_use]
pub fn inbox_tool_definitions() -> Vec<ToolDefinition> {
    agent_tool_definitions()
        .into_iter()
        .filter(|definition| is_inbox_tool(&definition.name))
        .collect()
}

/// The [`agent_tool_definitions`] only background agents offer: starting
/// them and stopping them.
#[must_use]
pub fn host_tool_definitions() -> Vec<ToolDefinition> {
    agent_tool_definitions()
        .into_iter()
        .filter(|definition| !is_inbox_tool(&definition.name))
        .collect()
}

/// Whether `name` is one of the [`inbox_tool_definitions`].
pub(crate) fn is_inbox_tool(name: &str) -> bool {
    matches!(
        name,
        SEND_TO_AGENT_TOOL_NAME | LIST_AGENTS_TOOL_NAME | GET_AGENT_STATE_TOOL_NAME
    )
}

/// Gives spawned agents the tools the terminal's agents have.
pub struct TerminalChildKernels {
    catalog: Arc<ToolCatalog>,
    tool_config: ToolConfig,
    background: Arc<dyn BackgroundProcessHook>,
    /// Where spawned agents' commands show what they print as they run.
    commands: Option<mpsc::Sender<UiCommand>>,
}

impl TerminalChildKernels {
    #[must_use]
    pub fn new(tool_config: ToolConfig, background: Arc<dyn BackgroundProcessHook>) -> Self {
        Self {
            catalog: Arc::new(ToolCatalog::new()),
            tool_config,
            background,
            commands: None,
        }
    }

    /// Show what spawned agents' commands print, as they run, in their
    /// chats through `commands`.
    #[must_use]
    pub fn with_command_output(mut self, commands: mpsc::Sender<UiCommand>) -> Self {
        self.commands = Some(commands);
        self
    }
}

impl ChildKernelFactory for TerminalChildKernels {
    fn build_child(&self, request: &ChildKernelRequest) -> ChildKernel {
        let mut resolver =
            aura_engine::executor::build_tool_resolver(&self.catalog, &self.tool_config, None)
                .with_background_process_hook(Arc::clone(&self.background));
        if let Some(commands) = &self.commands {
            let output = TerminalCommandOutput::new(commands.clone(), request.child_agent_id);
            resolver = resolver.with_command_output_hook(Arc::new(output));
        }
        let mut tools = self.catalog.executor_builtin_tools();
        tools.extend(agent_tool_definitions());
        ChildKernel { resolver, tools }
    }
}

/// Hands messages for a terminal agent to the event loop.
struct TerminalInbox {
    notices: NoticeSender,
}

#[async_trait]
impl ParentInbox for TerminalInbox {
    async fn deliver(&self, message: InboxMessage) -> Result<(), String> {
        let shown = if message.subject.is_empty() {
            format!("Message from {}", message.from_name)
        } else {
            format!("Message from {}: {}", message.from_name, message.subject)
        };
        let notice = Notice {
            to: message.to,
            shown,
            prompt: message.render(),
            inbox: None,
        };
        self.notices
            .send(notice)
            .map_err(|_| "the terminal is shutting down".to_string())
    }
}

/// The host, and what the terminal's agents share in it.
#[derive(Clone)]
pub struct TerminalAgents {
    host: AgentHost,
    scheduler: Arc<Scheduler>,
    factory: Arc<dyn ChildKernelFactory>,
}

impl TerminalAgents {
    #[must_use]
    pub fn new(
        host: AgentHost,
        scheduler: Arc<Scheduler>,
        factory: Arc<dyn ChildKernelFactory>,
    ) -> Self {
        Self {
            host,
            scheduler,
            factory,
        }
    }

    #[must_use]
    pub fn host(&self) -> &AgentHost {
        &self.host
    }

    /// Hooks for a terminal agent's tool executor.
    #[must_use]
    pub fn hooks_for(&self, agent: AgentId) -> Arc<AgentHooks> {
        Arc::new(self.host.hooks_for(agent))
    }

    /// Make `agent` a root that can spawn agents working in `folder` on
    /// `turn`'s model, in its mode, which they inherit along with its cache
    /// salt. Called before each of its turns, so a model or mode change
    /// reaches the agents it spawns next.
    pub fn register(
        &self,
        agent: AgentId,
        name: &str,
        folder: PathBuf,
        turn: &RootTurn<'_>,
        notices: NoticeSender,
    ) {
        self.scheduler
            .identity_registry()
            .register(agent, identity(turn));
        let mut env = AgentEnv::new(Arc::clone(&self.factory));
        env.workspace = Some((folder, true));
        self.host.register_root(RootAgent {
            agent_id: agent,
            name: name.to_string(),
            permissions: AgentPermissions::full_access(),
            tool_permissions: None,
            mode: turn.mode,
            model: turn.config.model.clone(),
            env,
            inbox: Some(Arc::new(TerminalInbox { notices })),
        });
    }
}

/// What a terminal agent's next turn gives the agents it spawns.
pub struct RootTurn<'a> {
    /// The turn's loop config: model, credentials and system prompt
    pub config: &'a AgentLoopConfig,
    /// The session mode spawns inherit
    pub mode: AgentMode,
    /// Prompt-cache scope of the agent's kernel, which its spawns share
    pub cache_salt: Option<&'a str>,
    /// Scratch folder of the agent's commands, which its spawns share
    pub run_scratch: Option<&'a aura_exec_tools::RunScratch>,
}

/// What agents spawned under a terminal agent inherit from its turn.
fn identity(turn: &RootTurn<'_>) -> AgentIdentity {
    let config = turn.config;
    AgentIdentity {
        reasoning_effort: config.user_thinking_effort,
        upstream_provider_family: config.upstream_provider_family.clone(),
        model: config.model.clone(),
        aura_org_id: config.aura_org_id.clone(),
        aura_session_id: config.aura_session_id.clone(),
        aura_agent_id: config.aura_agent_id.clone(),
        aura_project_id: config.aura_project_id.clone(),
        system_prompt: config.system_prompt.clone(),
        prompt_cache_key: None,
        prompt_cache_retention: None,
        request_kind: ModelRequestKind::Chat,
        max_context_tokens: config
            .max_context_tokens
            .and_then(|tokens| usize::try_from(tokens).ok())
            .unwrap_or(200_000),
        auth_token: config.auth_token.clone(),
        cache_salt: turn.cache_salt.map(str::to_string),
        run_scratch: turn.run_scratch.cloned(),
    }
}

#[cfg(test)]
mod tests;
