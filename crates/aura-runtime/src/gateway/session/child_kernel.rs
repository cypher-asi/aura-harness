//! Runtime-side [`ChildKernelFactory`]: gives agents spawned from a chat
//! session the same tool surface the session has (catalog tools, domain
//! tools, installed tools, devloop controls). The agent host attaches
//! the spawn / task / messaging hooks on top.

use std::path::PathBuf;
use std::sync::Arc;

use aura_core_types::{InstalledIntegrationDefinition, InstalledToolDefinition};
use aura_engine::executor;
use aura_engine::{ChildKernel, ChildKernelFactory, ChildKernelRequest};
use aura_tools::automaton_tools::AutomatonController;
use aura_tools::domain_tools::DomainToolExecutor;

/// Captured devloop-control wiring so children expose the same
/// automaton tools the parent session does.
struct AutomatonToolParams {
    controller: Arc<dyn AutomatonController>,
    project_id: String,
    workspace_root: Option<PathBuf>,
    auth_token: Option<String>,
}

/// Session-scoped [`ChildKernelFactory`].
pub(crate) struct SessionChildKernelFactory {
    catalog: Arc<aura_tools::ToolCatalog>,
    session_tool_config: aura_tools::ToolConfig,
    domain_exec: Option<Arc<DomainToolExecutor>>,
    installed_tools: Vec<InstalledToolDefinition>,
    installed_integrations: Vec<InstalledIntegrationDefinition>,
    automaton: Option<AutomatonToolParams>,
}

/// Inputs for [`SessionChildKernelFactory::new`], mirroring the
/// session state `build_kernel_with_config` already resolved.
pub(crate) struct SessionChildKernelFactoryParams {
    pub catalog: Arc<aura_tools::ToolCatalog>,
    pub session_tool_config: aura_tools::ToolConfig,
    pub domain_exec: Option<Arc<DomainToolExecutor>>,
    pub installed_tools: Vec<InstalledToolDefinition>,
    pub installed_integrations: Vec<InstalledIntegrationDefinition>,
    pub automaton_controller: Option<Arc<dyn AutomatonController>>,
    pub automaton_project_id: String,
    pub automaton_workspace_root: Option<PathBuf>,
    pub auth_token: Option<String>,
}

impl SessionChildKernelFactory {
    pub(crate) fn new(params: SessionChildKernelFactoryParams) -> Arc<Self> {
        let automaton = params
            .automaton_controller
            .map(|controller| AutomatonToolParams {
                controller,
                project_id: params.automaton_project_id,
                workspace_root: params.automaton_workspace_root,
                auth_token: params.auth_token,
            });
        Arc::new(Self {
            catalog: params.catalog,
            session_tool_config: params.session_tool_config,
            domain_exec: params.domain_exec,
            installed_tools: params.installed_tools,
            installed_integrations: params.installed_integrations,
            automaton,
        })
    }
}

impl ChildKernelFactory for SessionChildKernelFactory {
    fn build_child(&self, request: &ChildKernelRequest) -> ChildKernel {
        let mut resolver = executor::build_tool_resolver(
            &self.catalog,
            &self.session_tool_config,
            self.domain_exec.clone(),
        )
        .with_installed_tools(self.installed_tools.clone());

        if let Some(ref automaton) = self.automaton {
            for tool in aura_tools::automaton_tools::devloop_control_tools(
                automaton.controller.clone(),
                automaton.project_id.clone(),
                automaton.workspace_root.clone(),
                automaton.auth_token.clone(),
            ) {
                resolver.register(tool);
            }
        }

        let tools = crate::tool_permissions::effective_tool_definitions(
            &self.catalog,
            &self.session_tool_config,
            &self.installed_tools,
            &self.installed_integrations,
            &request.user_tool_defaults,
            Some(&request.tool_permissions),
            Some(&request.permissions),
        )
        .into_iter()
        .map(|(definition, _state)| definition)
        .collect();

        ChildKernel { resolver, tools }
    }
}
