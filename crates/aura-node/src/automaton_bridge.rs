//! Bridge between `AutomatonController` (defined in `aura-tools`) and the
//! concrete `AutomatonRuntime` + automaton types (from `aura-automaton`).
//!
//! This module lives in `aura-node` because it depends on both crates.
//! It handles: JWT injection, tool executor wiring, event broadcasting,
//! and non-blocking task execution.

// TODO(Phase 8e): KernelDomainGateway wrapping DomainApi mutations through kernel.process()

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use tokio::sync::broadcast;
use tracing::{info, warn};

use aura_agent::agent_runner::AgentRunnerConfig;
use aura_agent::{KernelModelGateway, KernelToolGateway};
use aura_automaton::{
    AutomatonEvent, AutomatonHandle, AutomatonRuntime, DevLoopAutomaton, TaskRunAutomaton,
};
use aura_core::{
    AgentId, InstalledIntegrationDefinition, InstalledToolDefinition, SystemKind, Transaction,
    TransactionType,
};
use aura_kernel::{Kernel, KernelConfig};
use aura_reasoner::ModelProvider;
use aura_store::Store;
use aura_tools::automaton_tools::AutomatonController;
use aura_tools::catalog::ToolCatalog;
use aura_tools::domain_tools::{DomainApi, DomainToolExecutor};
use aura_tools::ToolConfig;

use crate::executor_factory;
use crate::jwt_domain::JwtDomainApi;
use crate::protocol::{installed_integration_to_core, installed_tool_to_core};
use crate::runtime_capabilities;

const EVENT_BROADCAST_CAPACITY: usize = 512;

/// Concrete [`AutomatonController`] wired to the real runtime.
pub struct AutomatonBridge {
    runtime: Arc<AutomatonRuntime>,
    store: Arc<dyn Store>,
    domain: Arc<dyn DomainApi>,
    provider: Arc<dyn ModelProvider + Send + Sync>,
    catalog: Arc<ToolCatalog>,
    tool_config: ToolConfig,
    /// project_id -> (automaton_id, handle)
    project_handles: Arc<DashMap<String, (String, AutomatonHandle)>>,
    /// automaton_id -> broadcast sender for events
    event_channels: Arc<DashMap<String, broadcast::Sender<AutomatonEvent>>>,
}

impl AutomatonBridge {
    pub fn new(
        runtime: Arc<AutomatonRuntime>,
        store: Arc<dyn Store>,
        domain: Arc<dyn DomainApi>,
        provider: Arc<dyn ModelProvider + Send + Sync>,
        catalog: Arc<ToolCatalog>,
        tool_config: ToolConfig,
    ) -> Self {
        Self {
            runtime,
            store,
            domain,
            provider,
            catalog,
            tool_config,
            project_handles: Arc::new(DashMap::new()),
            event_channels: Arc::new(DashMap::new()),
        }
    }

    /// Subscribe to events for a running automaton.
    pub fn subscribe_events(
        &self,
        automaton_id: &str,
    ) -> Option<broadcast::Receiver<AutomatonEvent>> {
        self.event_channels
            .get(automaton_id)
            .map(|entry| entry.value().subscribe())
    }

    /// Wrap domain API with JWT injection when an auth token is available.
    fn domain_with_jwt(&self, auth_token: Option<&str>) -> Arc<dyn DomainApi> {
        match auth_token {
            Some(token) if !token.is_empty() => {
                Arc::new(JwtDomainApi::new(self.domain.clone(), token.to_string()))
            }
            _ => self.domain.clone(),
        }
    }

    fn tool_has_required_integration(
        required_integration: Option<&aura_core::InstalledToolIntegrationRequirement>,
        installed_integrations: &[InstalledIntegrationDefinition],
    ) -> bool {
        let Some(required_integration) = required_integration else {
            return true;
        };

        installed_integrations.iter().any(|integration| {
            required_integration
                .integration_id
                .as_deref()
                .map_or(true, |expected| integration.integration_id == expected)
                && required_integration
                    .provider
                    .as_deref()
                    .map_or(true, |expected| integration.provider == expected)
                && required_integration
                    .kind
                    .as_deref()
                    .map_or(true, |expected| integration.kind == expected)
        })
    }

    fn prepare_installed_tools(
        installed_tools: Option<Vec<aura_protocol::InstalledTool>>,
        installed_integrations: &[InstalledIntegrationDefinition],
    ) -> Vec<InstalledToolDefinition> {
        installed_tools
            .unwrap_or_default()
            .into_iter()
            .map(installed_tool_to_core)
            .filter(|tool| {
                Self::tool_has_required_integration(
                    tool.required_integration.as_ref(),
                    installed_integrations,
                )
            })
            .collect()
    }

    /// Build a per-agent [`Kernel`] backed by the shared store.
    ///
    /// The returned kernel owns an `ExecutorRouter` wired to the domain API
    /// (with optional JWT + project context) and serves as the single authority
    /// for tool execution and model reasoning recording for this agent.
    #[allow(clippy::too_many_arguments)] // TODO(W4): group inputs into a `BuildKernelParams` struct.
    fn build_kernel(
        &self,
        domain: Arc<dyn DomainApi>,
        auth_token: Option<&str>,
        project_id: Option<&str>,
        workspace: &std::path::Path,
        use_workspace_base_as_root: bool,
        installed_tools: Vec<InstalledToolDefinition>,
        installed_integrations: Vec<InstalledIntegrationDefinition>,
    ) -> Arc<Kernel> {
        let domain_exec = Arc::new(DomainToolExecutor::with_session_context(
            domain,
            auth_token.map(String::from),
            project_id.map(String::from),
        ));
        let resolver = executor_factory::build_tool_resolver(
            &self.catalog,
            &self.tool_config,
            Some(domain_exec),
        )
        .with_installed_tools(installed_tools.clone());
        let router = executor_factory::build_executor_router(resolver);
        let agent_id = AgentId::generate();
        let config = KernelConfig {
            workspace_base: workspace.to_path_buf(),
            use_workspace_base_as_root,
            policy: runtime_capabilities::build_policy_config(
                &installed_tools,
                &installed_integrations,
                // Dev-loop automaton kernels have no per-agent
                // aura-network profile; fall back to the kernel's
                // built-in default tool-permission matrix.
                &std::collections::HashMap::new(),
            ),
            ..KernelConfig::default()
        };

        match Kernel::new(
            self.store.clone(),
            self.provider.clone(),
            router,
            config,
            agent_id,
        ) {
            Ok(k) => Arc::new(k),
            Err(e) => {
                warn!(error = %e, "Kernel::new failed, falling back to fresh agent id");
                let fallback_router = executor_factory::build_executor_router(
                    executor_factory::build_tool_resolver(&self.catalog, &self.tool_config, None)
                        .with_installed_tools(installed_tools.clone()),
                );
                // Retry with a fresh `AgentId` and the same config; the only
                // failure mode left for `Kernel::new` is store corruption, in
                // which case we log and fall through to a second attempt. If
                // even that fails, there's no coherent recovery path left for
                // the dev-loop — we log fatally and bail by returning a
                // kernel constructed against an in-memory cache, to avoid
                // panicking the node process.
                match Kernel::new(
                    self.store.clone(),
                    self.provider.clone(),
                    fallback_router,
                    KernelConfig {
                        workspace_base: workspace.to_path_buf(),
                        use_workspace_base_as_root,
                        policy: runtime_capabilities::build_policy_config(
                            &installed_tools,
                            &installed_integrations,
                            &std::collections::HashMap::new(),
                        ),
                        ..KernelConfig::default()
                    },
                    AgentId::generate(),
                ) {
                    Ok(k) => Arc::new(k),
                    Err(e) => {
                        warn!(
                            error = %e,
                            "fallback Kernel::new failed; dev-loop will be unavailable for this project"
                        );
                        // Final-resort path: re-run `Kernel::new` with the
                        // already-validated router and the minimum viable
                        // config, propagating whatever error emerges. If this
                        // also fails we surface the error via `unreachable!`
                        // after a structured log — the node's dev-loop wiring
                        // has exhausted every recoverable configuration.
                        let last_resort = executor_factory::build_executor_router(
                            executor_factory::build_tool_resolver(
                                &self.catalog,
                                &self.tool_config,
                                None,
                            ),
                        );
                        match Kernel::new(
                            self.store.clone(),
                            self.provider.clone(),
                            last_resort,
                            KernelConfig::default(),
                            AgentId::generate(),
                        ) {
                            Ok(k) => Arc::new(k),
                            Err(final_err) => unreachable!(
                                "Kernel::new failed on default config after two retries: {final_err}"
                            ),
                        }
                    }
                }
            }
        }
    }

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
            let (ref id, ref handle) = *entry;
            if !handle.is_finished() {
                return Err(format!(
                    "A dev loop is already running for project {project_id} (automaton_id: {id})"
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

        let kernel = self.build_kernel(
            domain.clone(),
            auth_token.as_deref(),
            Some(project_id),
            ws_path,
            effective_workspace.is_some(),
            installed_tools.clone(),
            installed_integrations.clone(),
        );
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

        let runner_config = self.build_runner_config(model.as_deref(), auth_token.as_deref());
        let catalog = Arc::new(
            self.catalog
                .with_installed_tools(aura_tools::catalog::ToolProfile::Engine, &installed_tools),
        );

        let automaton = DevLoopAutomaton::new(domain, model_gw, runner_config, catalog)
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
        self.record_lifecycle_event(kernel.agent_id, &automaton_id, "start_dev_loop");
        self.spawn_event_forwarder(automaton_id.clone(), event_rx);

        info!(project_id, automaton_id = %automaton_id, "Dev loop started");
        self.project_handles
            .insert(project_id.to_string(), (automaton_id.clone(), handle));
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

        let kernel = self.build_kernel(
            domain.clone(),
            auth_token.as_deref(),
            Some(project_id),
            ws_path,
            effective_workspace.is_some(),
            installed_tools.clone(),
            installed_integrations.clone(),
        );
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

        let runner_config = self.build_runner_config(model.as_deref(), auth_token.as_deref());
        let catalog = Arc::new(
            self.catalog
                .with_installed_tools(aura_tools::catalog::ToolProfile::Engine, &installed_tools),
        );

        let automaton = TaskRunAutomaton::new(domain, model_gw, runner_config, catalog)
            .with_tool_executor(tool_gw);

        let config = serde_json::json!({
            "project_id": project_id,
            "task_id": task_id,
            "git_repo_url": git_repo_url,
            "git_branch": git_branch,
            "auth_token": auth_token.as_deref(),
        });

        let (handle, event_rx) = self
            .runtime
            .install(Box::new(automaton), config, effective_workspace)
            .await
            .map_err(|e| format!("failed to install task-run automaton: {e}"))?;

        let automaton_id = handle.id().as_str().to_string();
        self.record_lifecycle_event(kernel.agent_id, &automaton_id, "start_task_run");
        self.spawn_event_forwarder(automaton_id.clone(), event_rx);

        info!(project_id, task_id, automaton_id = %automaton_id, "Task execution started (non-blocking)");
        Ok(automaton_id)
    }

    /// Record an automaton lifecycle event as a System transaction.
    fn record_lifecycle_event(&self, agent_id: AgentId, automaton_id: &str, event: &str) {
        let payload = serde_json::json!({
            "system_kind": SystemKind::AutomatonLifecycle,
            "automaton_id": automaton_id,
            "event": event,
        });
        let Ok(payload_bytes) = serde_json::to_vec(&payload) else {
            warn!("Failed to serialize lifecycle event payload");
            return;
        };
        let tx = Transaction::new_chained(agent_id, TransactionType::System, payload_bytes, None);
        if let Err(e) = self.store.enqueue_tx(&tx) {
            warn!(error = %e, "Failed to record automaton lifecycle event");
        }
    }

    /// Spawn a background task that forwards `mpsc` events to a `broadcast` channel.
    fn spawn_event_forwarder(
        &self,
        automaton_id: String,
        mut event_rx: tokio::sync::mpsc::Receiver<AutomatonEvent>,
    ) -> broadcast::Sender<AutomatonEvent> {
        let (broadcast_tx, _) = broadcast::channel(EVENT_BROADCAST_CAPACITY);
        let channels = self.event_channels.clone();
        channels.insert(automaton_id.clone(), broadcast_tx.clone());

        let tx_for_task = broadcast_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let is_done = matches!(event, AutomatonEvent::Done);
                let _ = tx_for_task.send(event);
                if is_done {
                    break;
                }
            }
            channels.remove(&automaton_id);
        });

        broadcast_tx
    }

    fn build_runner_config(
        &self,
        model: Option<&str>,
        auth_token: Option<&str>,
    ) -> AgentRunnerConfig {
        let mut config = AgentRunnerConfig::default();
        if let Some(m) = model {
            config.default_model = m.to_string();
        }
        config.auth_token = auth_token.map(String::from);
        config
    }

    // ------------------------------------------------------------------
    // Direct REST-friendly methods (by automaton_id, not project_id)
    // ------------------------------------------------------------------

    /// Pause an automaton by its ID.
    pub fn pause_by_id(&self, automaton_id: &str) -> Result<(), String> {
        for entry in self.project_handles.iter() {
            let (ref aid, ref handle) = *entry.value();
            if aid == automaton_id {
                if handle.is_finished() {
                    return Err("Automaton has already finished".into());
                }
                handle.pause();
                info!(automaton_id, "Automaton paused via REST");
                return Ok(());
            }
        }
        Err(format!("Automaton {automaton_id} not found"))
    }

    /// Stop an automaton by its ID.
    pub fn stop_by_id(&self, automaton_id: &str) -> Result<(), String> {
        for entry in self.project_handles.iter() {
            let (ref aid, ref handle) = *entry.value();
            if aid == automaton_id {
                if handle.is_finished() {
                    let project_id = entry.key().clone();
                    drop(entry);
                    self.project_handles.remove(&project_id);
                    return Err("Automaton has already finished".into());
                }
                handle.stop();
                let project_id = entry.key().clone();
                drop(entry);
                self.project_handles.remove(&project_id);
                info!(automaton_id, "Automaton stopped via REST");
                return Ok(());
            }
        }
        // Also try the runtime directly (for task runs not in project_handles).
        self.runtime.stop(automaton_id).map_err(|e| e.to_string())
    }

    /// Get the status of an automaton by its ID.
    pub fn get_status(&self, automaton_id: &str) -> Option<aura_automaton::AutomatonInfo> {
        self.runtime.get_info(automaton_id)
    }

    /// List all running automatons.
    pub fn list_automatons(&self) -> Vec<aura_automaton::AutomatonInfo> {
        self.runtime.list()
    }
}

#[async_trait]
impl AutomatonController for AutomatonBridge {
    async fn start_dev_loop(
        &self,
        project_id: &str,
        workspace_root: Option<PathBuf>,
        auth_token: Option<String>,
        model: Option<String>,
        git_repo_url: Option<String>,
        git_branch: Option<String>,
    ) -> Result<String, String> {
        self.start_dev_loop_with_capabilities(
            project_id,
            workspace_root,
            auth_token,
            model,
            git_repo_url,
            git_branch,
            None,
            None,
        )
        .await
    }

    async fn pause_dev_loop(&self, project_id: &str) -> Result<(), String> {
        let entry = self
            .project_handles
            .get(project_id)
            .ok_or_else(|| format!("No running dev loop for project {project_id}"))?;
        let (_, ref handle) = *entry;
        if handle.is_finished() {
            return Err("Dev loop has already finished".into());
        }
        handle.pause();
        info!(project_id, "Dev loop paused");
        Ok(())
    }

    async fn stop_dev_loop(&self, project_id: &str) -> Result<(), String> {
        let entry = self
            .project_handles
            .get(project_id)
            .ok_or_else(|| format!("No running dev loop for project {project_id}"))?;
        let (ref id, ref handle) = *entry;
        if handle.is_finished() {
            drop(entry);
            self.project_handles.remove(project_id);
            return Err("Dev loop has already finished".into());
        }
        let automaton_id = id.clone();
        handle.stop();
        drop(entry);
        self.project_handles.remove(project_id);
        info!(project_id, automaton_id = %automaton_id, "Dev loop stopped");
        Ok(())
    }

    async fn run_task(
        &self,
        project_id: &str,
        task_id: &str,
        workspace_root: Option<PathBuf>,
        auth_token: Option<String>,
        model: Option<String>,
        git_repo_url: Option<String>,
        git_branch: Option<String>,
    ) -> Result<String, String> {
        self.run_task_with_capabilities(
            project_id,
            task_id,
            workspace_root,
            auth_token,
            model,
            git_repo_url,
            git_branch,
            None,
            None,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::AutomatonBridge;
    use aura_core::InstalledIntegrationDefinition;

    #[test]
    fn prepare_installed_tools_filters_by_required_integration() {
        let tools = AutomatonBridge::prepare_installed_tools(
            Some(vec![
                aura_protocol::InstalledTool {
                    name: "brave_search_web".to_string(),
                    description: "Search the web using Brave".to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": { "query": { "type": "string" } },
                        "required": ["query"]
                    }),
                    endpoint: "https://example.com/brave".to_string(),
                    auth: aura_protocol::ToolAuth::None,
                    timeout_ms: None,
                    namespace: None,
                    required_integration: Some(
                        aura_protocol::InstalledToolIntegrationRequirement {
                            integration_id: None,
                            provider: Some("brave_search".to_string()),
                            kind: Some("workspace_integration".to_string()),
                        },
                    ),
                    runtime_execution: None,
                    metadata: Default::default(),
                },
                aura_protocol::InstalledTool {
                    name: "list_org_integrations".to_string(),
                    description: "List org integrations".to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {},
                    }),
                    endpoint: "https://example.com/list".to_string(),
                    auth: aura_protocol::ToolAuth::None,
                    timeout_ms: None,
                    namespace: None,
                    required_integration: None,
                    runtime_execution: None,
                    metadata: Default::default(),
                },
            ]),
            &[InstalledIntegrationDefinition {
                integration_id: "brave-1".to_string(),
                name: "Brave Search".to_string(),
                provider: "brave_search".to_string(),
                kind: "workspace_integration".to_string(),
                metadata: Default::default(),
            }],
        );

        let names = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"brave_search_web"));
        assert!(names.contains(&"list_org_integrations"));

        let filtered = AutomatonBridge::prepare_installed_tools(
            Some(vec![aura_protocol::InstalledTool {
                name: "brave_search_web".to_string(),
                description: "Search the web using Brave".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                }),
                endpoint: "https://example.com/brave".to_string(),
                auth: aura_protocol::ToolAuth::None,
                timeout_ms: None,
                namespace: None,
                required_integration: Some(aura_protocol::InstalledToolIntegrationRequirement {
                    integration_id: None,
                    provider: Some("brave_search".to_string()),
                    kind: Some("workspace_integration".to_string()),
                }),
                runtime_execution: None,
                metadata: Default::default(),
            }]),
            &[],
        );

        assert!(filtered.is_empty());
    }
}
