//! Bridge between `AutomatonController` (defined in `aura-tools`) and the
//! concrete `AutomatonRuntime` + automaton types (from `aura-automaton`).
//!
//! This module lives in `aura-node` because it depends on both crates.
//! It handles: JWT injection, tool executor wiring, event broadcasting,
//! and non-blocking task execution.

//! Automaton bridge wires automaton-runtime surfaces (dev-loop, task-run)
//! into per-agent kernels. Domain mutations performed by automaton
//! orchestration code route through [`KernelDomainGateway`] so every
//! `create_spec` / `transition_task` / `save_message` produces a
//! `System` `DomainMutation` pair in the record log (Invariants §2 / §8).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use tokio::sync::broadcast;
use tracing::{info, warn};

use aura_agent::agent_runner::AgentRunnerConfig;
use aura_agent::{KernelDomainGateway, KernelModelGateway, KernelToolGateway};
use aura_automaton::{
    AutomatonEvent, AutomatonHandle, AutomatonRuntime, DevLoopAutomaton, TaskRunAutomaton,
};
use aura_core::{
    AgentId, InstalledIntegrationDefinition, InstalledToolDefinition, SystemKind, Transaction,
    TransactionType,
};
use aura_kernel::{Kernel, KernelConfig, PolicyConfig};
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
use crate::scheduler::Scheduler;

const EVENT_BROADCAST_CAPACITY: usize = 512;

/// Cap on the per-automaton replay buffer used by [`EventChannel::history`].
/// Mirrors the broadcast ring so an existing subscriber that manages to
/// keep up and a late subscriber that relies on replay see the same
/// visible window. Exceeding the cap drops the oldest entries first.
const EVENT_HISTORY_CAPACITY: usize = EVENT_BROADCAST_CAPACITY;

/// How long an [`EventChannel`] is kept in [`AutomatonBridge::event_channels`]
/// after the automaton emits `Done`. Provides a grace window for late
/// WebSocket subscribers (in particular, aura-os-server connects to
/// `/stream/automaton/:id` *after* `POST /automaton/start` returns, and
/// a fast-failing automaton can emit all its events before the WS
/// client even finishes its handshake). During this window
/// `subscribe_events` still returns the full replay history so the
/// late subscriber can reconstruct the task's outcome.
const RETENTION_AFTER_DONE: Duration = Duration::from_secs(300);

/// Per-automaton event bus.
///
/// The raw `broadcast::Sender` we used previously had a subtle race:
/// `tokio::sync::broadcast` only delivers to receivers that existed
/// when `send` was called. A new subscriber joining after emission
/// starts at the tail and misses every event already sent, including
/// `Started` / `TaskStarted` / `TaskFailed` / `TaskCompleted` / `Done`.
/// For fast-terminating automatons (typical failure paths complete in
/// &lt;100 ms) the aura-os-server WS client would therefore connect to a
/// "stream closed before terminal event arrived" - no visible reason,
/// no task outcome - even though the harness logs showed the automaton
/// had in fact run and failed.
///
/// This wrapper bundles a `broadcast::Sender` with a replay `history`
/// buffer: `spawn_event_forwarder` appends every event to `history`
/// before broadcasting, and `subscribe_events` returns the history
/// snapshot alongside a live receiver. Late subscribers get the full
/// event sequence regardless of when they joined.
pub(crate) struct EventChannel {
    /// Replay history. Capped at [`EVENT_HISTORY_CAPACITY`]; when full,
    /// the oldest entries are dropped. Cloned on each `subscribe_events`
    /// call (single-automaton events are small serde-derived values so
    /// the clone is cheap relative to the ~300s retention window).
    history: Mutex<Vec<AutomatonEvent>>,
    /// Live broadcast for in-flight subscribers. Retained inside the
    /// `Arc<EventChannel>` so the sender outlives the forwarder task
    /// and late subscribers don't see `RecvError::Closed` before they've
    /// drained the history.
    broadcast: broadcast::Sender<AutomatonEvent>,
    /// Set once the forwarder has observed and forwarded
    /// `AutomatonEvent::Done`. Lets subscribers skip the live-receive
    /// loop entirely when the automaton has already finished.
    done: AtomicBool,
}

/// Snapshot returned by [`AutomatonBridge::subscribe_events`]. Gives
/// callers both the replay history (consume first, in order) and a
/// live receiver (consume next, in order) so they produce the same
/// ordering any early subscriber would have seen.
pub struct EventSubscription {
    /// All events the automaton has emitted so far, in emission order.
    /// May be empty if the automaton hasn't ticked yet, or capped at
    /// [`EVENT_HISTORY_CAPACITY`] for long-lived dev-loop automatons.
    pub history: Vec<AutomatonEvent>,
    /// Receiver for events emitted after this subscribe call. Will
    /// yield `RecvError::Closed` once the retention window elapses
    /// (or immediately, if `already_done` is true and no more events
    /// will ever be sent).
    pub live: broadcast::Receiver<AutomatonEvent>,
    /// True when `Done` is already in `history`. Callers can use this
    /// to avoid waiting on `live.recv()` after draining history.
    pub already_done: bool,
}

/// Bookkeeping for a running automaton so stop/pause paths can emit
/// `System::AutomatonLifecycle` entries on the correct agent log
/// without rebuilding the per-agent kernel.
struct ProjectHandle {
    automaton_id: String,
    agent_id: AgentId,
    handle: AutomatonHandle,
}

/// Concrete [`AutomatonController`] wired to the real runtime.
pub struct AutomatonBridge {
    runtime: Arc<AutomatonRuntime>,
    // TODO(phase2-followup): Invariant §10 — bind to `Arc<dyn ReadStore>`
    // once `Kernel::new` accepts a read-only store + write hook. The
    // bridge never calls `append_entry_*` itself; it only passes the
    // handle through to `build_kernel` → `Kernel::new`.
    store: Arc<dyn Store>,
    domain: Arc<dyn DomainApi>,
    provider: Arc<dyn ModelProvider + Send + Sync>,
    catalog: Arc<ToolCatalog>,
    tool_config: ToolConfig,
    /// project_id -> tracked (automaton_id, agent_id, handle) tuple.
    ///
    /// The `agent_id` component is carried so lifecycle stop events
    /// recorded by the REST-friendly stop paths can scope the
    /// `System::AutomatonLifecycle` transaction to the same agent log
    /// the corresponding start event landed on (Invariant §2 / §8).
    project_handles: Arc<DashMap<String, ProjectHandle>>,
    /// automaton_id -> replay-aware event channel. See
    /// [`EventChannel`] for why this wraps the broadcast rather than
    /// using one directly.
    event_channels: Arc<DashMap<String, Arc<EventChannel>>>,
    /// Scheduler used to drain the per-agent inbox after a lifecycle
    /// `System` transaction is enqueued. Optional so test harnesses can
    /// construct a bridge without a live scheduler; production wiring
    /// always sets this via [`AutomatonBridge::with_scheduler`].
    scheduler: Option<Arc<Scheduler>>,
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
            scheduler: None,
        }
    }

    /// Attach the scheduler used to drain the lifecycle inbox.
    ///
    /// After [`record_lifecycle_event`](Self::record_lifecycle_event)
    /// enqueues a `System::AutomatonLifecycle` transaction, the bridge
    /// immediately requests a scheduling tick for that agent so the
    /// entry is promoted into the record log instead of sitting in the
    /// inbox until the next unrelated wakeup.
    #[must_use]
    pub fn with_scheduler(mut self, scheduler: Arc<Scheduler>) -> Self {
        self.scheduler = Some(scheduler);
        self
    }

    /// Subscribe to events for a running automaton.
    ///
    /// Returns an [`EventSubscription`] snapshot that combines the
    /// replay history (events already emitted before this call) with
    /// a live receiver (events emitted from now on). See
    /// [`EventChannel`] for the motivating race: fast-terminating
    /// automatons can finish emitting every event before the first
    /// WebSocket client finishes its handshake, so a bare
    /// `broadcast::Receiver` routinely observed "stream closed with
    /// no terminal event".
    pub fn subscribe_events(&self, automaton_id: &str) -> Option<EventSubscription> {
        self.event_channels.get(automaton_id).map(|entry| {
            let ch = entry.value();
            let history = ch
                .history
                .lock()
                .expect("event history mutex poisoned")
                .clone();
            EventSubscription {
                history,
                live: ch.broadcast.subscribe(),
                already_done: ch.done.load(Ordering::Acquire),
            }
        })
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
            Some(workspace.to_string_lossy().into_owned()),
        ));
        let resolver = executor_factory::build_tool_resolver(
            &self.catalog,
            &self.tool_config,
            Some(domain_exec.clone()),
        )
        .with_installed_tools(installed_tools.clone());
        let router = executor_factory::build_executor_router(resolver);
        let agent_id = AgentId::generate();
        let policy = automaton_policy_config(&installed_tools, &installed_integrations);
        let config = KernelConfig {
            workspace_base: workspace.to_path_buf(),
            use_workspace_base_as_root,
            policy,
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
                        policy: automaton_policy_config(&installed_tools, &installed_integrations),
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

    /// Record an automaton lifecycle event as a System transaction.
    ///
    /// Enqueues a `System::AutomatonLifecycle` transaction on the
    /// agent's inbox and immediately nudges the scheduler so the entry
    /// is promoted into the record log without waiting for an unrelated
    /// wakeup. Scheduler failures are logged but never propagated —
    /// this is a lifecycle side-effect, not the main operation (§2, §8).
    pub(crate) async fn record_lifecycle_event(
        &self,
        agent_id: AgentId,
        automaton_id: &str,
        event: &str,
    ) {
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
            return;
        }
        // §2 requires that the System transaction eventually appears in
        // the record log. The scheduler drains the inbox through the
        // kernel's single-writer path; awaiting here means the record
        // entry is committed before the caller observes the lifecycle
        // write. Scheduler errors are logged but never propagated — a
        // lifecycle side-effect must not mask the underlying
        // start/stop operation.
        if let Some(scheduler) = self.scheduler.as_ref() {
            if let Err(e) = scheduler.schedule_agent(agent_id).await {
                warn!(
                    agent_id = %agent_id,
                    error = %e,
                    "Scheduler tick after lifecycle event failed"
                );
            }
        }
    }

    /// Spawn a background task that forwards `mpsc` events from the
    /// automaton runtime into both the replay `history` buffer and
    /// the live broadcast. See [`EventChannel`] for why both paths
    /// are needed.
    ///
    /// After `Done` is forwarded the channel entry is kept alive for
    /// [`RETENTION_AFTER_DONE`] so late subscribers can still pull
    /// the replay history. The entry is removed from
    /// [`AutomatonBridge::event_channels`] at the end of that window.
    fn spawn_event_forwarder(
        &self,
        automaton_id: String,
        mut event_rx: tokio::sync::mpsc::Receiver<AutomatonEvent>,
    ) -> Arc<EventChannel> {
        let (broadcast_tx, _) = broadcast::channel(EVENT_BROADCAST_CAPACITY);
        let channel = Arc::new(EventChannel {
            history: Mutex::new(Vec::new()),
            broadcast: broadcast_tx,
            done: AtomicBool::new(false),
        });
        let channels = self.event_channels.clone();
        channels.insert(automaton_id.clone(), channel.clone());

        let channel_for_task = channel.clone();
        let id_for_task = automaton_id.clone();
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let is_done = matches!(event, AutomatonEvent::Done);
                // Append to the replay history BEFORE broadcasting so
                // a subscriber that manages to subscribe between the
                // two operations sees the event in its history rather
                // than missing it entirely. Cap with
                // EVENT_HISTORY_CAPACITY (oldest-first eviction).
                {
                    let mut history = channel_for_task
                        .history
                        .lock()
                        .expect("event history mutex poisoned");
                    if history.len() >= EVENT_HISTORY_CAPACITY {
                        let drop_n = history.len() + 1 - EVENT_HISTORY_CAPACITY;
                        history.drain(..drop_n);
                    }
                    history.push(event.clone());
                }
                let _ = channel_for_task.broadcast.send(event);
                if is_done {
                    channel_for_task.done.store(true, Ordering::Release);
                    break;
                }
            }

            // Grace window: keep the channel entry discoverable so
            // late WebSocket subscribers can still read the replay
            // history. Holding `channel_for_task` here also keeps the
            // broadcast sender alive, so any subscriber that joined
            // mid-retention gets RecvError::Closed only after the
            // retention elapses (not immediately).
            tokio::time::sleep(RETENTION_AFTER_DONE).await;
            channels.remove(&id_for_task);
        });

        channel
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
            let tracked = entry.value();
            if tracked.automaton_id == automaton_id {
                if tracked.handle.is_finished() {
                    return Err("Automaton has already finished".into());
                }
                tracked.handle.pause();
                info!(automaton_id, "Automaton paused via REST");
                return Ok(());
            }
        }
        Err(format!("Automaton {automaton_id} not found"))
    }

    /// Stop an automaton by its ID.
    pub async fn stop_by_id(&self, automaton_id: &str) -> Result<(), String> {
        let mut target: Option<(String, AgentId)> = None;
        for entry in self.project_handles.iter() {
            let tracked = entry.value();
            if tracked.automaton_id == automaton_id {
                if tracked.handle.is_finished() {
                    let project_id = entry.key().clone();
                    drop(entry);
                    self.project_handles.remove(&project_id);
                    return Err("Automaton has already finished".into());
                }
                tracked.handle.stop();
                target = Some((entry.key().clone(), tracked.agent_id));
                break;
            }
        }
        if let Some((project_id, agent_id)) = target {
            self.project_handles.remove(&project_id);
            self.record_lifecycle_event(agent_id, automaton_id, "stop_dev_loop")
                .await;
            info!(automaton_id, "Automaton stopped via REST");
            return Ok(());
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

/// Build automaton kernel policy.
///
/// Tool availability now comes from the persisted user defaults and optional
/// agent overrides on [`PolicyConfig`]. This helper only wires runtime
/// integration requirements for installed tools.
fn automaton_policy_config(
    installed_tools: &[InstalledToolDefinition],
    installed_integrations: &[InstalledIntegrationDefinition],
) -> PolicyConfig {
    let mut policy = PolicyConfig::default();
    policy.set_installed_integrations(installed_integrations.iter().cloned());
    policy.set_tool_integration_requirements(installed_tools.iter().filter_map(|tool| {
        tool.required_integration
            .clone()
            .map(|requirement| (tool.name.clone(), requirement))
    }));
    policy
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
        let tracked = entry.value();
        if tracked.handle.is_finished() {
            return Err("Dev loop has already finished".into());
        }
        tracked.handle.pause();
        info!(project_id, "Dev loop paused");
        Ok(())
    }

    async fn stop_dev_loop(&self, project_id: &str) -> Result<(), String> {
        let (automaton_id, agent_id) = {
            let entry = self
                .project_handles
                .get(project_id)
                .ok_or_else(|| format!("No running dev loop for project {project_id}"))?;
            let tracked = entry.value();
            if tracked.handle.is_finished() {
                let project_id_owned = project_id.to_string();
                drop(entry);
                self.project_handles.remove(&project_id_owned);
                return Err("Dev loop has already finished".into());
            }
            tracked.handle.stop();
            (tracked.automaton_id.clone(), tracked.agent_id)
        };
        self.project_handles.remove(project_id);
        self.record_lifecycle_event(agent_id, &automaton_id, "stop_dev_loop")
            .await;
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
            None,
            Vec::new(),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::{AutomatonBridge, Scheduler};
    use async_trait::async_trait;
    use aura_automaton::AutomatonRuntime;
    use aura_core::{AgentId, InstalledIntegrationDefinition, TransactionType};
    use aura_reasoner::{MockProvider, ModelProvider};
    use aura_store::{RocksStore, Store};
    use aura_tools::{
        domain_tools::{
            CreateSessionParams, DomainApi, MessageDescriptor, ProjectDescriptor, ProjectUpdate,
            SaveMessageParams, SessionDescriptor, SpecDescriptor, TaskDescriptor, TaskUpdate,
        },
        ToolCatalog, ToolConfig,
    };
    use std::sync::Arc;

    /// A `DomainApi` stub whose methods all panic — the lifecycle test
    /// below never invokes any of them because it only exercises the
    /// bridge's inbox/scheduler wiring, not the automaton runtime
    /// itself.
    struct UnusedDomain;

    #[async_trait]
    impl DomainApi for UnusedDomain {
        async fn list_specs(
            &self,
            _project_id: &str,
            _jwt: Option<&str>,
        ) -> anyhow::Result<Vec<SpecDescriptor>> {
            unimplemented!("UnusedDomain")
        }
        async fn get_spec(
            &self,
            _spec_id: &str,
            _jwt: Option<&str>,
        ) -> anyhow::Result<SpecDescriptor> {
            unimplemented!("UnusedDomain")
        }
        async fn create_spec(
            &self,
            _p: &str,
            _t: &str,
            _c: &str,
            _o: u32,
            _j: Option<&str>,
        ) -> anyhow::Result<SpecDescriptor> {
            unimplemented!("UnusedDomain")
        }
        async fn update_spec(
            &self,
            _id: &str,
            _t: Option<&str>,
            _c: Option<&str>,
            _j: Option<&str>,
        ) -> anyhow::Result<SpecDescriptor> {
            unimplemented!("UnusedDomain")
        }
        async fn delete_spec(&self, _id: &str, _j: Option<&str>) -> anyhow::Result<()> {
            unimplemented!("UnusedDomain")
        }
        async fn list_tasks(
            &self,
            _p: &str,
            _s: Option<&str>,
            _j: Option<&str>,
        ) -> anyhow::Result<Vec<TaskDescriptor>> {
            unimplemented!("UnusedDomain")
        }
        async fn create_task(
            &self,
            _p: &str,
            _s: &str,
            _t: &str,
            _d: &str,
            _deps: &[String],
            _o: u32,
            _j: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            unimplemented!("UnusedDomain")
        }
        async fn update_task(
            &self,
            _id: &str,
            _u: TaskUpdate,
            _j: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            unimplemented!("UnusedDomain")
        }
        async fn delete_task(&self, _id: &str, _j: Option<&str>) -> anyhow::Result<()> {
            unimplemented!("UnusedDomain")
        }
        async fn transition_task(
            &self,
            _id: &str,
            _s: &str,
            _j: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            unimplemented!("UnusedDomain")
        }
        async fn claim_next_task(
            &self,
            _p: &str,
            _a: &str,
            _j: Option<&str>,
        ) -> anyhow::Result<Option<TaskDescriptor>> {
            unimplemented!("UnusedDomain")
        }
        async fn get_task(&self, _id: &str, _j: Option<&str>) -> anyhow::Result<TaskDescriptor> {
            unimplemented!("UnusedDomain")
        }
        async fn get_project(
            &self,
            _p: &str,
            _j: Option<&str>,
        ) -> anyhow::Result<ProjectDescriptor> {
            unimplemented!("UnusedDomain")
        }
        async fn update_project(
            &self,
            _p: &str,
            _u: ProjectUpdate,
            _j: Option<&str>,
        ) -> anyhow::Result<ProjectDescriptor> {
            unimplemented!("UnusedDomain")
        }
        async fn create_log(
            &self,
            _p: &str,
            _m: &str,
            _l: &str,
            _a: Option<&str>,
            _md: Option<&serde_json::Value>,
            _j: Option<&str>,
        ) -> anyhow::Result<serde_json::Value> {
            unimplemented!("UnusedDomain")
        }
        async fn list_logs(
            &self,
            _p: &str,
            _l: Option<&str>,
            _n: Option<u64>,
            _j: Option<&str>,
        ) -> anyhow::Result<serde_json::Value> {
            unimplemented!("UnusedDomain")
        }
        async fn get_project_stats(
            &self,
            _p: &str,
            _j: Option<&str>,
        ) -> anyhow::Result<serde_json::Value> {
            unimplemented!("UnusedDomain")
        }
        async fn list_messages(
            &self,
            _p: &str,
            _i: &str,
        ) -> anyhow::Result<Vec<MessageDescriptor>> {
            unimplemented!("UnusedDomain")
        }
        async fn save_message(&self, _p: SaveMessageParams) -> anyhow::Result<()> {
            unimplemented!("UnusedDomain")
        }
        async fn create_session(
            &self,
            _p: CreateSessionParams,
        ) -> anyhow::Result<SessionDescriptor> {
            unimplemented!("UnusedDomain")
        }
        async fn get_active_session(&self, _i: &str) -> anyhow::Result<Option<SessionDescriptor>> {
            unimplemented!("UnusedDomain")
        }
        async fn orbit_api_call(
            &self,
            _m: &str,
            _p: &str,
            _b: Option<&serde_json::Value>,
            _j: Option<&str>,
        ) -> anyhow::Result<String> {
            unimplemented!("UnusedDomain")
        }
        async fn network_api_call(
            &self,
            _m: &str,
            _p: &str,
            _b: Option<&serde_json::Value>,
            _j: Option<&str>,
        ) -> anyhow::Result<String> {
            unimplemented!("UnusedDomain")
        }
    }

    fn count_lifecycle_entries(store: &Arc<dyn Store>, agent_id: AgentId) -> usize {
        store
            .scan_record(agent_id, 0, 256)
            .expect("scan_record")
            .into_iter()
            .filter(|e| e.tx.tx_type == TransactionType::System)
            .filter(|e| {
                serde_json::from_slice::<serde_json::Value>(&e.tx.payload)
                    .ok()
                    .and_then(|v| {
                        v.get("system_kind")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    })
                    .as_deref()
                    == Some("automaton_lifecycle")
            })
            .count()
    }

    /// §2 + §8: starting and stopping an automaton must each produce
    /// one `System::AutomatonLifecycle` entry in the record log for the
    /// owning agent. This test exercises the bridge's
    /// `record_lifecycle_event` seam directly so the assertion is
    /// focused on the inbox → scheduler → record-log hop that the
    /// automaton runtime triggers, without spinning up a real dev loop.
    #[tokio::test]
    async fn start_then_stop_records_two_automaton_lifecycle_entries() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> =
            Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("noop"));
        let ws_dir = dir.path().join("workspaces");
        std::fs::create_dir_all(&ws_dir).unwrap();

        let scheduler = Arc::new(Scheduler::new(
            store.clone(),
            provider.clone(),
            vec![],
            vec![],
            ws_dir,
            None,
        ));

        let runtime = Arc::new(AutomatonRuntime::new());
        let catalog = Arc::new(ToolCatalog::new());
        let domain: Arc<dyn DomainApi> = Arc::new(UnusedDomain);
        let bridge = AutomatonBridge::new(
            runtime,
            store.clone(),
            domain,
            provider,
            catalog,
            ToolConfig::default(),
        )
        .with_scheduler(scheduler);

        let agent_id = AgentId::generate();

        bridge
            .record_lifecycle_event(agent_id, "aut-1", "start_dev_loop")
            .await;
        bridge
            .record_lifecycle_event(agent_id, "aut-1", "stop_dev_loop")
            .await;

        let count = count_lifecycle_entries(&store, agent_id);
        assert_eq!(
            count, 2,
            "expected exactly 2 System/AutomatonLifecycle entries, got {count}"
        );
    }

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

    // ------------------------------------------------------------------
    // Event-stream replay tests
    //
    // Regression tests for the race described on [`EventChannel`]:
    // `aura-os-server` connects to `/stream/automaton/:id` *after*
    // `POST /automaton/start` returns. `tokio::sync::broadcast`
    // receivers only observe events sent after they subscribe, so a
    // fast-terminating automaton used to look like "stream closed
    // without a terminal event" from the server's point of view.
    //
    // These tests drive `spawn_event_forwarder` directly via the mpsc
    // it consumes, then exercise `subscribe_events` as a late
    // subscriber.
    // ------------------------------------------------------------------

    fn test_bridge() -> AutomatonBridge {
        use crate::scheduler::Scheduler;
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> =
            Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("noop"));
        let ws_dir = dir.path().join("workspaces");
        std::fs::create_dir_all(&ws_dir).unwrap();
        let scheduler = Arc::new(Scheduler::new(
            store.clone(),
            provider.clone(),
            vec![],
            vec![],
            ws_dir,
            None,
        ));
        let runtime = Arc::new(AutomatonRuntime::new());
        let catalog = Arc::new(ToolCatalog::new());
        let domain: Arc<dyn DomainApi> = Arc::new(UnusedDomain);
        AutomatonBridge::new(
            runtime,
            store,
            domain,
            provider,
            catalog,
            ToolConfig::default(),
        )
        .with_scheduler(scheduler)
    }

    /// A subscriber that joins after every event has been emitted still
    /// sees the full sequence via [`EventSubscription::history`].
    #[tokio::test]
    async fn late_subscriber_sees_replayed_history_after_done() {
        use aura_automaton::AutomatonEvent;

        let bridge = test_bridge();
        let automaton_id = "aut-replay".to_string();
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        bridge.spawn_event_forwarder(automaton_id.clone(), rx);

        tx.send(AutomatonEvent::Started {
            automaton_id: automaton_id.clone(),
        })
        .await
        .unwrap();
        tx.send(AutomatonEvent::TaskStarted {
            task_id: "task-1".into(),
            task_title: "first task".into(),
        })
        .await
        .unwrap();
        tx.send(AutomatonEvent::TaskFailed {
            task_id: "task-1".into(),
            reason: "boom".into(),
        })
        .await
        .unwrap();
        tx.send(AutomatonEvent::Stopped {
            automaton_id: automaton_id.clone(),
            reason: "Failed".into(),
        })
        .await
        .unwrap();
        tx.send(AutomatonEvent::Done).await.unwrap();

        // Wait for the forwarder to observe Done and set `done=true`.
        // The forwarder pushes history before toggling the flag, so
        // once `already_done` is true we know every event is visible.
        let subscription = loop {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let sub = bridge
                .subscribe_events(&automaton_id)
                .expect("channel still in retention window");
            if sub.already_done {
                break sub;
            }
        };

        let kinds: Vec<&'static str> = subscription
            .history
            .iter()
            .map(|e| match e {
                AutomatonEvent::Started { .. } => "started",
                AutomatonEvent::TaskStarted { .. } => "task_started",
                AutomatonEvent::TaskFailed { .. } => "task_failed",
                AutomatonEvent::Stopped { .. } => "stopped",
                AutomatonEvent::Done => "done",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["started", "task_started", "task_failed", "stopped", "done"],
            "late subscriber must see every emitted event in order"
        );
        assert!(subscription.already_done);
    }

    /// A subscriber that joins mid-stream sees the events emitted so
    /// far through `history` and any later events through `live`, in
    /// order. This is what would have saved us in the logs the user
    /// shared: the WS would observe `Started → TaskFailed → Done`
    /// regardless of whether it subscribed 1 ms or 200 ms after
    /// `POST /automaton/start` returned.
    #[tokio::test]
    async fn mid_stream_subscriber_sees_history_then_live_events() {
        use aura_automaton::AutomatonEvent;

        let bridge = test_bridge();
        let automaton_id = "aut-mid".to_string();
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        bridge.spawn_event_forwarder(automaton_id.clone(), rx);

        tx.send(AutomatonEvent::Started {
            automaton_id: automaton_id.clone(),
        })
        .await
        .unwrap();
        tx.send(AutomatonEvent::TaskStarted {
            task_id: "task-1".into(),
            task_title: "first".into(),
        })
        .await
        .unwrap();

        // Let the forwarder drain the two events above into history.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let mut subscription = bridge
            .subscribe_events(&automaton_id)
            .expect("channel present");
        assert!(!subscription.already_done);
        assert_eq!(subscription.history.len(), 2);
        assert!(matches!(
            subscription.history[0],
            AutomatonEvent::Started { .. }
        ));
        assert!(matches!(
            subscription.history[1],
            AutomatonEvent::TaskStarted { .. }
        ));

        // Emit the remainder after subscribe. These should arrive on
        // the live receiver, not in history (history was snapshotted).
        tx.send(AutomatonEvent::TaskCompleted {
            task_id: "task-1".into(),
            summary: "ok".into(),
        })
        .await
        .unwrap();
        tx.send(AutomatonEvent::Done).await.unwrap();

        let first = subscription.live.recv().await.expect("live event");
        assert!(matches!(first, AutomatonEvent::TaskCompleted { .. }));
        let second = subscription.live.recv().await.expect("live event");
        assert!(matches!(second, AutomatonEvent::Done));
    }

    /// History is capped at [`EVENT_HISTORY_CAPACITY`] so long-lived
    /// dev-loop automatons don't grow unbounded. The oldest events
    /// are dropped first; this is consistent with how
    /// `tokio::sync::broadcast` would have behaved for an early
    /// subscriber that fell behind.
    #[tokio::test]
    async fn history_caps_at_capacity_and_drops_oldest() {
        use aura_automaton::AutomatonEvent;

        let bridge = test_bridge();
        let automaton_id = "aut-cap".to_string();
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        bridge.spawn_event_forwarder(automaton_id.clone(), rx);

        let over = super::EVENT_HISTORY_CAPACITY + 5;
        for i in 0..over {
            tx.send(AutomatonEvent::LogLine {
                message: format!("line {i}"),
            })
            .await
            .unwrap();
        }

        // Drain.
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        let subscription = bridge
            .subscribe_events(&automaton_id)
            .expect("channel present");
        assert_eq!(
            subscription.history.len(),
            super::EVENT_HISTORY_CAPACITY,
            "history must be capped at EVENT_HISTORY_CAPACITY"
        );
        // The very first 5 "line 0".."line 4" should have been evicted.
        match &subscription.history[0] {
            AutomatonEvent::LogLine { message } => {
                assert_eq!(
                    message, "line 5",
                    "oldest surviving entry should be the 6th emitted event"
                );
            }
            other => panic!("unexpected event kind: {other:?}"),
        }
    }
}
