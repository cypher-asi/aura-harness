//! The agent harness shared by the terminal, the desktop app and `aura -p`:
//! tool settings, the model provider, kernel policy and the agent hub.

use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::event_loop;
use crate::file_checkpoints::FileCheckpoints;
use crate::long_run::{add_long_run_tools, WebTools};
use crate::project_instructions::{InstructionSettings, ProjectInstructions};
use crate::session_helpers;

use anyhow::Context;
use aura_agent::{ProcessManager, ProcessManagerConfig};
use aura_agent_kernel::{
    ExecutorRouter, Kernel, KernelConfig, RecordObserver, ToolApprovalPrompter,
};
use aura_context_skills::{SkillLoader, SkillManager};
use aura_core_types::{AgentId, AgentPermissions, Transaction};
use aura_exec_tools::todo::TodoWriteTool;
use aura_exec_tools::{BackgroundProcessHook, SubagentDispatchHook, ToolConfig, ToolExecutor};
use aura_gate::live::GateLive;
use aura_model::ModelProvider;
use aura_ui::{ApprovalLevel, PermissionMode, UiCommand};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// How long a background command may run before it is killed. Long
/// enough for a dev server to outlive a working day.
const BACKGROUND_COMMAND_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000;

/// Tool settings, the model-facing tool list and the web tools for
/// terminal-style agents: the autonomous dev-loop tools plus planning,
/// background jobs, `web_fetch` and `web_search`. The web tools must be
/// registered on each agent's executor (see [`build_hub`]) to match the
/// list.
pub(crate) fn agent_tools(
    allow_unrestricted_full_access: bool,
) -> (ToolConfig, Vec<aura_model::ToolDefinition>, WebTools) {
    let mut tool_config = ToolConfig::for_autonomous_dev_loop();
    tool_config.sync_threshold_ms = crate::long_run::COMMAND_TIMEOUT_MS;
    if allow_unrestricted_full_access || unrestricted_full_access_from_env() {
        tool_config.command.allow_unrestricted_full_access = true;
        warn!("unrestricted full-access command allowlist bypass enabled by operator config");
    }
    if tool_config.command.enabled {
        info!(
            binary_allowlist = ?tool_config.command.binary_allowlist,
            allow_shell = tool_config.command.allow_shell,
            allow_unrestricted_full_access = tool_config.command.allow_unrestricted_full_access,
            "run_command enabled"
        );
    }
    let (_, mut tools) = session_helpers::build_executor_router_with_config(&tool_config);
    let web_tools = WebTools::from_env();
    add_long_run_tools(&mut tools, &web_tools);
    (tool_config, tools, web_tools)
}

/// The post-edit check settings from `AURA_CHECK_COMMAND` and
/// `AURA_CHECK_TIMEOUT` (the command may also come from `AGENTS.md`).
pub(crate) fn post_edit_check_from_env() -> aura_agent::PostEditCheckConfig {
    aura_agent::PostEditCheckConfig::from_lookup(|name| std::env::var(name).ok())
}

/// The model provider named by `--provider`: `mock` for tests and offline
/// use; anything else routes `local/*` models to the local server
/// (Ollama), `claude/*` and `chatgpt/*` to the linked plan, and everything
/// else to aura-router.
pub(crate) fn build_provider(
    name: &str,
    auth: &Arc<aura_auth::AuthManager>,
) -> anyhow::Result<aura_model::ProviderSelection> {
    let resolver: Arc<dyn aura_auth::CredentialResolver> = auth.clone();
    match name {
        "mock" => Ok(aura_model::mock_provider()),
        _ => aura_model::routed_provider(Some(resolver)),
    }
    .context("building model provider")
}

/// Everything [`build_hub`] wires together.
pub(crate) struct HubSetup {
    pub data_dir: PathBuf,
    /// Where tools run for agents with no project folder
    pub workspace_root: PathBuf,
    pub registry: AgentRegistry,
    pub definitions: AgentDefinitions,
    pub store: Arc<aura_store_db::RocksStore>,
    pub provider: Arc<dyn ModelProvider + Send + Sync>,
    pub tool_config: ToolConfig,
    pub base: event_loop::TurnBase,
    /// The settings Gate routes with
    pub gate: aura_config::GateConfig,
    /// Kernel policy for every agent: mode and tool permissions
    pub policy: aura_agent_kernel::PolicyConfig,
    /// Sees every record entry of every agent, including background ones
    pub record_observer: Option<Arc<dyn RecordObserver>>,
    /// Which agent is on screen; shared with `record_observer`
    pub record_panel: event_loop::RecordPanelAgent,
    /// Joins record entries into run traces; fed by `record_observer`
    pub record_router: Option<Arc<aura_agent_trace::RecordRouter>>,
    /// `AURA_HOME`, where run traces go; `None` records none
    pub trace_home: Option<PathBuf>,
    /// Where agents' chats and panels go
    pub commands: mpsc::Sender<UiCommand>,
    pub long_run: crate::long_run::LongRunSettings,
    /// Runs `run_command`'s background commands and reports their exits;
    /// from [`background_process_manager`]
    pub process_manager: Arc<ProcessManager>,
    /// Offer `spawn_agent` and the tools that talk to background agents
    pub background_agents: bool,
    /// Work mode and approval level agents start in; spawned agents take
    /// their parent's. Should match what `policy` was built for.
    pub permissions: event_loop::Permissions,
    pub instructions: ProjectInstructions,
    /// Asks the user about tool calls the policy wants approved. Without
    /// it the kernel denies those calls, which `aura -p` relies on.
    pub approvals: Option<Arc<event_loop::TerminalApprovals>>,
    /// Registered on every agent's executor; from [`agent_tools`]
    pub web_tools: WebTools,
    /// Run after each step that edits files; results go to `commands`
    pub post_edit_check: aura_agent::PostEditCheckConfig,
    /// Keep kernel snapshots where run traces can replay them, instead of
    /// the kernel's default store
    pub trace_snapshots: bool,
}

/// What [`build_hub`] built.
pub(crate) struct Harness {
    pub hub: Arc<event_loop::AgentHub>,
    /// Every agent's background commands
    pub background: Arc<dyn BackgroundProcessHook>,
}

/// The process manager for background commands, with the channel their
/// completions arrive on.
pub(crate) fn background_process_manager() -> (Arc<ProcessManager>, mpsc::Receiver<Transaction>) {
    let (process_tx, process_rx) = mpsc::channel::<Transaction>(100);
    let manager = Arc::new(ProcessManager::new(
        process_tx,
        ProcessManagerConfig {
            max_async_timeout_ms: BACKGROUND_COMMAND_TIMEOUT_MS,
            ..ProcessManagerConfig::default()
        },
    ));
    (manager, process_rx)
}

/// Names of every tool an agent may be offered: `tools` plus `task`,
/// which the subagent dispatcher adds.
pub(crate) fn policy_tool_names(tools: &[aura_model::ToolDefinition]) -> Vec<String> {
    let mut names: Vec<String> = tools.iter().map(|tool| tool.name.clone()).collect();
    names.push(aura_exec_tools::agents::task::TASK_TOOL_NAME.to_string());
    names
}

/// Kernel policy for agents under `permissions`, the same table the hub
/// applies when an agent switches mode or a spawn takes its parent's.
pub(crate) fn mode_policy(
    permissions: event_loop::Permissions,
    tool_names: &[String],
) -> aura_agent_kernel::PolicyConfig {
    let mut policy = aura_agent_kernel::PolicyConfig::default()
        .with_mode(permissions.mode.agent_mode())
        .with_user_default(event_loop::modes::user_defaults(permissions, tool_names));
    policy.approval = permissions.approval;
    policy
}

/// Project instructions (`AGENTS.md`, `CLAUDE.md`, Cursor rules) as the
/// environment configures them, with the user's own from the Aura home.
pub(crate) fn project_instructions() -> ProjectInstructions {
    ProjectInstructions::new(InstructionSettings::from_env(
        aura_config::AuraHome::resolve()
            .ok()
            .map(|home| home.path)
            .as_deref(),
    ))
}

/// The agent hub: registry, permanent agents, the Processes tab's table,
/// the host for background agents, and a kernel factory whose kernels run
/// the tools in `setup` under its policy, hand background commands to its
/// process manager, and checkpoint their sessions under `data_dir`.
pub(crate) fn build_hub(mut setup: HubSetup) -> Harness {
    let gate = start_gate(&setup);
    let kernel_config = kernel_config(&mut setup, &gate);
    let wiring = wire_agents(&mut setup, gate, kernel_config);
    assemble_hub(setup, wiring)
}

/// What [`build_hub`] wires before it assembles the hub.
struct Wiring {
    gate: Arc<GateLive>,
    kernel_config: KernelConfig,
    traces: Arc<event_loop::Traces>,
    processes: Arc<event_loop::ProcessTable>,
    background: Arc<dyn BackgroundProcessHook>,
    agents: Option<event_loop::TerminalAgents>,
}

/// Gate, rebuilding its tables and catching up on compare agents in the
/// background, logging its attempts under the data dir.
fn start_gate(setup: &HubSetup) -> Arc<GateLive> {
    let gate = Arc::new(GateLive::new(setup.gate));
    gate.rebuild_in_background(
        setup.store.clone() as Arc<dyn aura_store_db::ReadStore>,
        setup
            .registry
            .agents()
            .iter()
            .map(|a| a.agent_id())
            .collect(),
    );
    gate.log_attempts_to(aura_gate::attempts::AttemptLog::in_data_dir(
        &setup.data_dir,
    ));
    let compares = setup.registry.compares();
    gate.catch_up_in_background(
        setup.store.clone() as Arc<dyn aura_store_db::ReadStore>,
        compares
            .iter()
            .map(|a| (a.agent_id(), a.name.clone()))
            .collect(),
        crate::session_checkpoint::Checkpoints::new(&setup.data_dir),
    );
    gate
}

/// The config every agent's kernel starts from. Takes the policy and the
/// record observer out of `setup`; Gate sees the records too.
fn kernel_config(setup: &mut HubSetup, gate: &Arc<GateLive>) -> KernelConfig {
    let observers = setup.record_observer.take().into_iter();
    let observer = event_loop::FanoutObserver(
        observers
            .chain([Arc::clone(gate) as Arc<dyn RecordObserver>])
            .collect(),
    );
    let prompter = setup
        .approvals
        .clone()
        .map(|approvals| approvals as Arc<dyn ToolApprovalPrompter>);
    let mut kernel_config = KernelConfig {
        workspace_base: setup.workspace_root.clone(),
        use_workspace_base_as_root: true,
        policy: std::mem::take(&mut setup.policy),
        tool_approval_prompter: prompter,
        record_observer: Some(Arc::new(observer)),
        ..KernelConfig::default()
    };
    if let Some(store) = setup
        .trace_snapshots
        .then(|| aura_config::AuraHome::resolve().ok())
        .flatten()
        .and_then(|home| aura_agent_trace::kernel_snapshot_store(&home.path).ok())
    {
        kernel_config.snapshot_store = store;
    }
    kernel_config
}

/// The host and scheduler background agents run on.
fn agent_host(
    setup: &HubSetup,
    config: &KernelConfig,
) -> (aura_engine::AgentHost, Arc<aura_engine::Scheduler>) {
    let kernel_store = setup.store.clone() as Arc<dyn aura_store_db::Store>;
    let mut scheduler = aura_engine::Scheduler::new(
        Arc::clone(&kernel_store),
        Arc::clone(&setup.provider),
        Vec::new(),
        setup.base.tools.clone(),
        setup.workspace_root.clone(),
        None,
    );
    if let Some(observer) = &config.record_observer {
        scheduler = scheduler.with_record_observer(Arc::clone(observer));
    }
    let scheduler = Arc::new(scheduler.with_snapshot_store(Arc::clone(&config.snapshot_store)));
    let host_config = aura_engine::AgentHostConfig::default();
    let host = aura_engine::AgentHost::new(kernel_store, Arc::clone(&scheduler), host_config);
    (host, scheduler)
}

/// Run traces, the Processes tab's table, background commands and the
/// background-agent host. Takes the trace settings out of `setup`.
fn wire_agents(setup: &mut HubSetup, gate: Arc<GateLive>, kernel_config: KernelConfig) -> Wiring {
    let host = setup
        .background_agents
        .then(|| agent_host(setup, &kernel_config));
    let trace_store = aura_store_db::TraceStore::for_store(&setup.store);
    let traces = event_loop::Traces::new(
        setup.trace_home.take(),
        Some(Arc::new(trace_store)),
        setup.record_router.take(),
        setup.commands.clone(),
    );
    if let Some((host, _)) = &host {
        host.set_turn_observer(Arc::clone(&traces) as Arc<dyn aura_engine::TurnObserver>);
    }
    let manager = &setup.process_manager;
    let processes = event_loop::ProcessTable::new(
        host.as_ref().map(|(host, _)| host.clone()),
        Some(Arc::clone(manager)),
    );
    let background: Arc<dyn BackgroundProcessHook> = Arc::new(event_loop::BackgroundCommands::new(
        Arc::clone(manager),
        setup.store.clone() as Arc<dyn aura_store_db::ReadStore>,
        setup.data_dir.join("processes"),
        &processes,
    ));
    let agents = host.map(|(host, scheduler)| {
        let tool_config = setup.tool_config.clone();
        let kernels = event_loop::TerminalChildKernels::new(tool_config, Arc::clone(&background))
            .with_command_output(setup.commands.clone());
        event_loop::TerminalAgents::new(host, scheduler, Arc::new(kernels))
    });
    Wiring {
        gate,
        kernel_config,
        traces,
        processes,
        background,
        agents,
    }
}

/// The hub over `setup` and `wiring`, with its kernel factory.
fn assemble_hub(setup: HubSetup, wiring: Wiring) -> Harness {
    let background = wiring.background;
    let checkpoints = crate::session_checkpoint::Checkpoints::new(&setup.data_dir);
    let kernels = KernelParts {
        store: setup.store.clone() as Arc<dyn aura_store_db::Store>,
        provider: setup.provider,
        config: wiring.kernel_config,
        tool_config: setup.tool_config,
        background: Arc::clone(&background),
        files: FileCheckpoints::new(&setup.data_dir),
        commands: setup.commands.clone(),
        agents: wiring.agents.clone(),
        approvals: setup.approvals.clone(),
        web_tools: setup.web_tools,
    };
    let check_commands = setup.commands.clone();
    let skills = SkillLoader::with_defaults(Some(setup.workspace_root), None);
    let hub = event_loop::AgentHub::new(
        event_loop::AgentHubParts {
            registry: setup.registry,
            definitions: setup.definitions,
            skills: SkillManager::new(skills),
            base: setup.base,
            commands: setup.commands,
            long_run: setup.long_run,
            mode: setup.permissions.mode,
            approval: setup.permissions.approval,
            approvals: setup.approvals,
            instructions: setup.instructions,
            processes: wiring.processes,
            agents: wiring.agents,
            record_panel: setup.record_panel,
            gate: wiring.gate,
            traces: wiring.traces,
            channels: Some(aura_store_db::ChannelStore::for_store(&setup.store)),
        },
        move |dispatcher| {
            kernels
                .into_factory(setup.store, dispatcher)
                .with_checkpoints(checkpoints)
                .with_post_edit_check(setup.post_edit_check, check_commands)
        },
    );
    Harness { hub, background }
}

/// What the kernel factory builds each agent's kernel from.
struct KernelParts {
    store: Arc<dyn aura_store_db::Store>,
    provider: Arc<dyn ModelProvider + Send + Sync>,
    config: KernelConfig,
    tool_config: ToolConfig,
    background: Arc<dyn BackgroundProcessHook>,
    files: FileCheckpoints,
    commands: mpsc::Sender<UiCommand>,
    agents: Option<event_loop::TerminalAgents>,
    approvals: Option<Arc<event_loop::TerminalApprovals>>,
    web_tools: WebTools,
}

impl KernelParts {
    /// The kernel factory over `store`, whose kernels hand their `task`
    /// calls to `dispatcher`.
    fn into_factory(
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
        if let Some(agents) = &self.agents {
            let hooks = agents.hooks_for(agent_id);
            executor = executor
                .with_spawn_hook(hooks.clone())
                .with_agent_control_hook(hooks.clone())
                .with_agent_read_hook(hooks);
        }
        executor.register(Box::new(TodoWriteTool));
        self.web_tools.register(&mut executor);
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

/// The permission mode terminal agents start in: `--mode`, else
/// `AURA_TUI_MODE`, else the daemon default (`AURA_DEFAULT_MODE`).
pub(crate) fn initial_permission_mode(
    requested: Option<aura_core_modes::AgentMode>,
) -> PermissionMode {
    if let Some(mode) = requested {
        return PermissionMode::from_agent_mode(mode);
    }
    if let Ok(raw) = std::env::var("AURA_TUI_MODE") {
        match PermissionMode::parse(&raw) {
            Ok(mode) => return mode,
            Err(e) => warn!(error = %e, "Ignoring AURA_TUI_MODE"),
        }
    }
    PermissionMode::from_agent_mode(aura_config::session_mode(None))
}

/// The approval level terminal agents start at: `--approval`, else
/// `AURA_TUI_APPROVAL`, else `yolo`. Never remembered from an earlier
/// session. The kernel's own default stays `ask`, so only surfaces that
/// ask for `yolo` confine their commands.
pub(crate) fn initial_approval_level(requested: Option<ApprovalLevel>) -> ApprovalLevel {
    if let Some(level) = requested {
        return level;
    }
    if let Ok(raw) = std::env::var("AURA_TUI_APPROVAL") {
        match ApprovalLevel::parse(&raw) {
            Some(level) => return level,
            None => warn!(value = %raw, "Ignoring unknown AURA_TUI_APPROVAL"),
        }
    }
    ApprovalLevel::Yolo
}

fn unrestricted_full_access_from_env() -> bool {
    std::env::var("AURA_ALLOW_UNRESTRICTED_FULL_ACCESS").is_ok_and(|value| {
        let value = value.trim();
        value == "1" || value.eq_ignore_ascii_case("true")
    })
}
