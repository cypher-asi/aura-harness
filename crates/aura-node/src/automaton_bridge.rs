//! Bridge between `AutomatonController` (defined in `aura-tools`) and the
//! concrete `AutomatonRuntime` + automaton types (from `aura-automaton`).
//!
//! This module lives in `aura-node` because it depends on both crates.
//! It handles: JWT injection, tool executor wiring, event broadcasting,
//! and non-blocking task execution.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use tokio::sync::broadcast;
use tracing::info;

use aura_agent::agent_runner::AgentRunnerConfig;
use aura_agent::KernelToolExecutor;
use aura_automaton::{
    AutomatonEvent, AutomatonHandle, AutomatonRuntime, DevLoopAutomaton, TaskRunAutomaton,
};
use aura_core::AgentId;
use aura_kernel::ExecutorRouter;
use aura_reasoner::ModelProvider;
use aura_tools::automaton_tools::AutomatonController;
use aura_tools::catalog::ToolCatalog;
use aura_tools::domain_tools::{DomainApi, DomainToolExecutor};
use aura_tools::{ToolConfig, ToolResolver};

use crate::jwt_domain::JwtDomainApi;

const EVENT_BROADCAST_CAPACITY: usize = 512;

/// Concrete [`AutomatonController`] wired to the real runtime.
pub struct AutomatonBridge {
    runtime: Arc<AutomatonRuntime>,
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
        domain: Arc<dyn DomainApi>,
        provider: Arc<dyn ModelProvider + Send + Sync>,
        catalog: Arc<ToolCatalog>,
        tool_config: ToolConfig,
    ) -> Self {
        Self {
            runtime,
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

    /// Build a `KernelToolExecutor` that automatons use for file/command tools.
    fn build_tool_executor(
        &self,
        domain: Arc<dyn DomainApi>,
        auth_token: Option<&str>,
        project_id: Option<&str>,
        workspace: &std::path::Path,
    ) -> Arc<KernelToolExecutor> {
        let mut resolver = ToolResolver::new(self.catalog.clone(), self.tool_config.clone());
        let domain_exec = Arc::new(DomainToolExecutor::with_session_context(
            domain,
            auth_token.map(String::from),
            project_id.map(String::from),
        ));
        resolver = resolver.with_domain_executor(domain_exec);

        let mut router = ExecutorRouter::new();
        router.add_executor(Arc::new(resolver));

        Arc::new(KernelToolExecutor::new(
            router,
            AgentId::generate(),
            workspace.to_path_buf(),
        ))
    }

    /// Spawn a background task that forwards `mpsc` events to a `broadcast` channel.
    fn spawn_event_forwarder(
        &self,
        automaton_id: String,
        mut event_rx: tokio::sync::mpsc::UnboundedReceiver<AutomatonEvent>,
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
        let tool_executor = if let Some(ref ws) = effective_workspace {
            self.build_tool_executor(domain.clone(), auth_token.as_deref(), Some(project_id), ws)
        } else {
            self.build_tool_executor(
                domain.clone(),
                auth_token.as_deref(),
                Some(project_id),
                std::path::Path::new("."),
            )
        };

        let runner_config = self.build_runner_config(model.as_deref(), auth_token.as_deref());

        let automaton = DevLoopAutomaton::new(
            domain,
            self.provider.clone(),
            runner_config,
            self.catalog.clone(),
        )
        .with_tool_executor(tool_executor);

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
        self.spawn_event_forwarder(automaton_id.clone(), event_rx);

        info!(project_id, automaton_id = %automaton_id, "Dev loop started");
        self.project_handles
            .insert(project_id.to_string(), (automaton_id.clone(), handle));
        Ok(automaton_id)
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
        let domain = self.domain_with_jwt(auth_token.as_deref());
        let effective_workspace = workspace_root.clone();
        let tool_executor = if let Some(ref ws) = effective_workspace {
            self.build_tool_executor(domain.clone(), auth_token.as_deref(), Some(project_id), ws)
        } else {
            self.build_tool_executor(
                domain.clone(),
                auth_token.as_deref(),
                Some(project_id),
                std::path::Path::new("."),
            )
        };

        let runner_config = self.build_runner_config(model.as_deref(), auth_token.as_deref());

        let automaton = TaskRunAutomaton::new(
            domain,
            self.provider.clone(),
            runner_config,
            self.catalog.clone(),
        )
        .with_tool_executor(tool_executor);

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
        self.spawn_event_forwarder(automaton_id.clone(), event_rx);

        info!(project_id, task_id, automaton_id = %automaton_id, "Task execution started (non-blocking)");
        Ok(automaton_id)
    }
}
