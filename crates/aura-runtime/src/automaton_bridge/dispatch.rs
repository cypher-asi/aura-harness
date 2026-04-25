//! Public entry-points that install automatons into the runtime.
//!
//! These are the methods `AutomatonController::start_dev_loop` /
//! `run_task` (via `mod.rs`) ultimately delegate to. They handle:
//!
//! 1. Per-project re-entrancy checks (only one dev loop per project).
//! 2. Tool/integration filtering (delegated to
//!    [`AutomatonBridge::prepare_installed_tools`] in [`super::build`]).
//! 3. Per-agent kernel construction (delegated to
//!    [`AutomatonBridge::build_kernel`] in [`super::build`]).
//! 4. Recording runtime capabilities for downstream debugging.
//! 5. Wiring the gateway domain so automaton-driven mutations land in
//!    the record log as `System::DomainMutation` (Invariant §2 / §8).
//! 6. Installing the automaton, recording the lifecycle event, and
//!    spawning the replay-aware event forwarder.

use std::path::PathBuf;
use std::sync::Arc;

use aura_agent::{KernelDomainGateway, KernelModelGateway, KernelToolGateway};
use aura_automaton::{DevLoopAutomaton, TaskRunAutomaton};
use aura_reasoner::ModelProvider;
use aura_tools::domain_tools::DomainApi;
use tracing::info;

use crate::protocol::installed_integration_to_core;
use crate::runtime_capabilities;

use super::{AutomatonBridge, ProjectHandle};

impl AutomatonBridge {
    #[allow(clippy::too_many_arguments)] // TODO(W4): collapse dev-loop kickoff args.
    pub(crate) async fn start_dev_loop_with_capabilities(
        &self,
        project_id: &str,
        workspace_root: Option<PathBuf>,
        auth_token: Option<String>,
        model: Option<String>,
        git_repo_url: Option<String>,
        git_branch: Option<String>,
        installed_tools: Option<Vec<aura_protocol::InstalledTool>>,
        installed_integrations: Option<Vec<aura_protocol::InstalledIntegration>>,
    ) -> Result<String, String> {
        if let Some(entry) = self.project_handles.get(project_id) {
            let tracked = entry.value();
            if !tracked.handle.is_finished() {
                return Err(format!(
                    "A dev loop is already running for project {project_id} (automaton_id: {})",
                    tracked.automaton_id
                ));
            }
            drop(entry);
            self.project_handles.remove(project_id);
        }

        let domain = self.domain_with_jwt(auth_token.as_deref());
        let effective_workspace = workspace_root.clone();
        let ws_path = effective_workspace
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("."));
        let installed_integrations = installed_integrations
            .unwrap_or_default()
            .into_iter()
            .map(installed_integration_to_core)
            .collect::<Vec<_>>();
        let installed_tools =
            Self::prepare_installed_tools(installed_tools, &installed_integrations);

        let kernel = self
            .build_kernel(
                domain.clone(),
                auth_token.as_deref(),
                Some(project_id),
                ws_path,
                effective_workspace.is_some(),
                installed_tools.clone(),
                installed_integrations.clone(),
            )
            .map_err(|e| format!("failed to build dev loop kernel: {e}"))?;
        if let Err(e) = runtime_capabilities::record_runtime_capabilities(
            &kernel,
            "automaton",
            None,
            &installed_tools,
            &installed_integrations,
        )
        .await
        {
            return Err(format!(
                "failed to record dev loop runtime capabilities: {e}"
            ));
        }
        let model_gw: Arc<dyn ModelProvider> = Arc::new(KernelModelGateway::new(kernel.clone()));
        let tool_gw: Arc<dyn aura_agent::AgentToolExecutor> =
            Arc::new(KernelToolGateway::new(kernel.clone()));
        // Wrap the domain so mutations driven by automaton orchestration
        // (not the LLM tool loop) route through `kernel.process_direct`
        // and produce `SystemKind::DomainMutation` record entries. The
        // raw `domain` is still used inside `build_kernel` for the
        // `DomainToolExecutor`, whose mutations are captured via
        // `ToolExecution` entries by the kernel itself.
        let gateway_domain: Arc<dyn DomainApi> =
            Arc::new(KernelDomainGateway::new(domain.clone(), kernel.clone()));

        let runner_config = self.build_runner_config(model.as_deref(), auth_token.as_deref());
        let catalog = Arc::new(
            self.catalog
                .with_installed_tools(aura_tools::catalog::ToolProfile::Engine, &installed_tools),
        );

        let automaton = DevLoopAutomaton::new(gateway_domain, model_gw, runner_config, catalog)
            .with_tool_executor(tool_gw);

        let config = serde_json::json!({
            "project_id": project_id,
            "git_repo_url": git_repo_url,
            "git_branch": git_branch,
            "auth_token": auth_token.as_deref(),
        });

        let (handle, event_rx) = self
            .runtime
            .install(Box::new(automaton), config, effective_workspace)
            .await
            .map_err(|e| format!("failed to install dev-loop automaton: {e}"))?;

        let automaton_id = handle.id().as_str().to_string();
        self.record_lifecycle_event(kernel.agent_id, &automaton_id, "start_dev_loop")
            .await;
        self.spawn_event_forwarder(automaton_id.clone(), event_rx);

        info!(project_id, automaton_id = %automaton_id, "Dev loop started");
        self.project_handles.insert(
            project_id.to_string(),
            ProjectHandle {
                automaton_id: automaton_id.clone(),
                agent_id: kernel.agent_id,
                handle,
            },
        );
        Ok(automaton_id)
    }

    #[allow(clippy::too_many_arguments)] // TODO(W4): collapse task-runner args.
    pub(crate) async fn run_task_with_capabilities(
        &self,
        project_id: &str,
        task_id: &str,
        workspace_root: Option<PathBuf>,
        auth_token: Option<String>,
        model: Option<String>,
        git_repo_url: Option<String>,
        git_branch: Option<String>,
        installed_tools: Option<Vec<aura_protocol::InstalledTool>>,
        installed_integrations: Option<Vec<aura_protocol::InstalledIntegration>>,
        prior_failure: Option<String>,
        work_log: Vec<String>,
    ) -> Result<String, String> {
        let domain = self.domain_with_jwt(auth_token.as_deref());
        let effective_workspace = workspace_root.clone();
        let ws_path = effective_workspace
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("."));
        let installed_integrations = installed_integrations
            .unwrap_or_default()
            .into_iter()
            .map(installed_integration_to_core)
            .collect::<Vec<_>>();
        let installed_tools =
            Self::prepare_installed_tools(installed_tools, &installed_integrations);

        let kernel = self
            .build_kernel(
                domain.clone(),
                auth_token.as_deref(),
                Some(project_id),
                ws_path,
                effective_workspace.is_some(),
                installed_tools.clone(),
                installed_integrations.clone(),
            )
            .map_err(|e| format!("failed to build task runtime kernel: {e}"))?;
        if let Err(e) = runtime_capabilities::record_runtime_capabilities(
            &kernel,
            "automaton",
            None,
            &installed_tools,
            &installed_integrations,
        )
        .await
        {
            return Err(format!("failed to record task runtime capabilities: {e}"));
        }
        let model_gw: Arc<dyn ModelProvider> = Arc::new(KernelModelGateway::new(kernel.clone()));
        let tool_gw: Arc<dyn aura_agent::AgentToolExecutor> =
            Arc::new(KernelToolGateway::new(kernel.clone()));
        let gateway_domain: Arc<dyn DomainApi> =
            Arc::new(KernelDomainGateway::new(domain.clone(), kernel.clone()));

        let runner_config = self.build_runner_config(model.as_deref(), auth_token.as_deref());
        let catalog = Arc::new(
            self.catalog
                .with_installed_tools(aura_tools::catalog::ToolProfile::Engine, &installed_tools),
        );

        let automaton = TaskRunAutomaton::new(gateway_domain, model_gw, runner_config, catalog)
            .with_tool_executor(tool_gw);

        let config = serde_json::json!({
            "project_id": project_id,
            "task_id": task_id,
            "git_repo_url": git_repo_url,
            "git_branch": git_branch,
            "auth_token": auth_token.as_deref(),
            "prior_failure": prior_failure,
            "work_log": work_log,
        });

        let (handle, event_rx) = self
            .runtime
            .install(Box::new(automaton), config, effective_workspace)
            .await
            .map_err(|e| format!("failed to install task-run automaton: {e}"))?;

        let automaton_id = handle.id().as_str().to_string();
        self.record_lifecycle_event(kernel.agent_id, &automaton_id, "start_task_run")
            .await;
        self.spawn_event_forwarder(automaton_id.clone(), event_rx);

        info!(project_id, task_id, automaton_id = %automaton_id, "Task execution started (non-blocking)");
        Ok(automaton_id)
    }
}
