//! The kernel each agent of the hub runs on.

use crate::event_loop;
use crate::file_checkpoints::FileCheckpoints;
use crate::long_run::WebTools;
use aura_agent_kernel::{ExecutorRouter, Kernel, KernelConfig};
use aura_core_types::{AgentId, AgentPermissions};
use aura_exec_tools::todo::TodoWriteTool;
use aura_exec_tools::{BackgroundProcessHook, SubagentDispatchHook, ToolConfig, ToolExecutor};
use aura_model::ModelProvider;
use aura_ui::UiCommand;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

/// What the kernel factory builds each agent's kernel from.
pub(super) struct KernelParts {
    pub(super) store: Arc<dyn aura_store_db::Store>,
    pub(super) provider: Arc<dyn ModelProvider + Send + Sync>,
    pub(super) config: KernelConfig,
    pub(super) tool_config: ToolConfig,
    pub(super) background: Arc<dyn BackgroundProcessHook>,
    pub(super) files: FileCheckpoints,
    pub(super) commands: mpsc::Sender<UiCommand>,
    pub(super) agents: Option<event_loop::TerminalAgents>,
    pub(super) approvals: Option<Arc<event_loop::TerminalApprovals>>,
    pub(super) web_tools: WebTools,
    /// The hub, once built: agents reach their peers and `assign` through it.
    pub(super) hub: event_loop::HubLink,
}

impl KernelParts {
    /// The kernel factory over `store`, whose kernels hand their `task`
    /// calls to `dispatcher`.
    pub(super) fn into_factory(
        self,
        store: Arc<aura_store_db::RocksStore>,
        dispatcher: Arc<dyn SubagentDispatchHook>,
    ) -> event_loop::KernelFactory {
        let files = self.files.clone();
        let build = Box::new(move |agent_id, parent_chain, folder| {
            self.kernel(&dispatcher, agent_id, parent_chain, folder)
        });
        event_loop::KernelFactory::new(store, build).with_file_checkpoints(files)
    }

    /// Register the tools only the app has: todos, `assign` and
    /// `load_skill`.
    fn register_own_tools(&self, executor: &mut ToolExecutor) {
        executor.register(Box::new(TodoWriteTool));
        executor.register(Box::new(event_loop::AssignTool::new(self.hub.clone())));
        executor.register(Box::new(aura_exec_tools::skills::LoadSkillTool::new(
            Arc::new(event_loop::DefinitionSkills::new(self.hub.clone())),
        )));
    }

    /// The kernel for `agent_id`, spawned by `parent_chain` (nearest
    /// first), with its tools run in `folder` (`None`: the launch folder).
    fn kernel(
        &self,
        dispatcher: &Arc<dyn SubagentDispatchHook>,
        agent_id: AgentId,
        parent_chain: Vec<AgentId>,
        folder: Option<PathBuf>,
    ) -> anyhow::Result<Kernel> {
        let chain = std::iter::once(agent_id)
            .chain(parent_chain.iter().copied())
            .collect();
        let file_hook = self.files.hook(chain, Some(self.commands.clone()));
        let command_output =
            event_loop::TerminalCommandOutput::new(self.commands.clone(), agent_id);
        let mut executor = ToolExecutor::new(self.tool_config.clone())
            .with_caller_permissions(AgentPermissions::full_access())
            .with_parent_chain(parent_chain)
            .with_subagent_dispatch_hook(Arc::clone(dispatcher))
            .with_background_process_hook(Arc::clone(&self.background))
            .with_file_change_hook(file_hook)
            .with_command_output_hook(Arc::new(command_output));
        let host_hooks = self
            .agents
            .as_ref()
            .map(|agents| agents.hooks_for(agent_id));
        if let Some(hooks) = &host_hooks {
            executor = executor.with_spawn_hook(hooks.clone());
        }
        let peers = Arc::new(event_loop::PeerHooks::new(
            self.hub.clone(),
            agent_id,
            host_hooks,
        ));
        executor = executor
            .with_agent_control_hook(peers.clone())
            .with_agent_read_hook(peers);
        self.register_own_tools(&mut executor);
        self.web_tools.register(&mut executor);
        if let Some(integrations) = crate::integrations::get() {
            integrations.register_tools(&mut executor, &agent_id.to_hex(), None);
        }
        let mut executor_router = ExecutorRouter::new();
        executor_router.add_executor(Arc::new(executor));
        let mut config = self.config.clone();
        if let Some(folder) = folder {
            config.workspace_base = folder;
        }
        if let Some(approvals) = &self.approvals {
            approvals.set_folder(agent_id, config.workspace_base.clone());
        }
        Ok(Kernel::new(
            Arc::clone(&self.store),
            Arc::clone(&self.provider),
            executor_router,
            config,
            agent_id,
        )?)
    }
}
