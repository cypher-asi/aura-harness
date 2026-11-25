//! The agent harness shared by the terminal, the desktop app and `aura -p`:
//! tool settings, the model provider, kernel policy and the agent hub.

use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::event_loop;
use crate::long_run::{add_long_run_tools, WebTools};
use crate::project_instructions::{InstructionSettings, ProjectInstructions};
use crate::session_helpers;

use anyhow::Context;
use aura_agent::{ProcessManager, ProcessManagerConfig};
use aura_agent_kernel::{
    ExecutorRouter, Kernel, KernelConfig, RecordObserver, ToolApprovalPrompter,
};
use aura_context_skills::{SkillLoader, SkillManager};
use aura_core_types::{AgentPermissions, Transaction};
use aura_exec_tools::todo::TodoWriteTool;
use aura_exec_tools::{BackgroundProcessHook, ToolConfig, ToolExecutor};
use aura_model::ModelProvider;
use aura_ui_terminal::{ApprovalLevel, PermissionMode, UiCommand};
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
pub(crate) fn build_hub(setup: HubSetup) -> Harness {
    let HubSetup {
        data_dir,
        workspace_root,
        registry,
        definitions,
        store,
        provider,
        tool_config,
        base,
        gate,
        policy,
        record_observer,
        record_panel,
        record_router,
        trace_home,
        commands,
        long_run,
        process_manager,
        background_agents,
        permissions,
        instructions,
        approvals,
        web_tools,
        post_edit_check,
        trace_snapshots,
    } = setup;
    let gate = Arc::new(aura_gate::live::GateLive::new(gate));
    gate.rebuild_in_background(
        store.clone() as Arc<dyn aura_store_db::ReadStore>,
        registry.agents().iter().map(|a| a.agent_id()).collect(),
    );
    gate.log_attempts_to(aura_gate::attempts::AttemptLog::in_data_dir(&data_dir));
    gate.catch_up_in_background(
        store.clone() as Arc<dyn aura_store_db::ReadStore>,
        registry
            .compares()
            .iter()
            .map(|a| (a.agent_id(), a.name.clone()))
            .collect(),
        crate::session_checkpoint::Checkpoints::new(&data_dir),
    );
    let record_observer: Option<Arc<dyn RecordObserver>> =
        Some(Arc::new(event_loop::FanoutObserver(
            record_observer
                .into_iter()
                .chain([Arc::clone(&gate) as Arc<dyn RecordObserver>])
                .collect(),
        )));
    let mut kernel_config = KernelConfig {
        workspace_base: workspace_root.clone(),
        use_workspace_base_as_root: true,
        policy,
        tool_approval_prompter: approvals
            .clone()
            .map(|approvals| approvals as Arc<dyn ToolApprovalPrompter>),
        record_observer: record_observer.clone(),
        ..KernelConfig::default()
    };
    if let Some(store) = trace_snapshots
        .then(|| aura_config::AuraHome::resolve().ok())
        .flatten()
        .and_then(|home| aura_agent_trace::kernel_snapshot_store(&home.path).ok())
    {
        kernel_config.snapshot_store = store;
    }
    let kernel_store = store.clone() as Arc<dyn aura_store_db::Store>;

    let host = background_agents.then(|| {
        let mut scheduler = aura_engine::Scheduler::new(
            Arc::clone(&kernel_store),
            Arc::clone(&provider),
            Vec::new(),
            base.tools.clone(),
            workspace_root.clone(),
            None,
        );
        if let Some(observer) = &record_observer {
            scheduler = scheduler.with_record_observer(Arc::clone(observer));
        }
        let scheduler =
            Arc::new(scheduler.with_snapshot_store(Arc::clone(&kernel_config.snapshot_store)));
        let host = aura_engine::AgentHost::new(
            Arc::clone(&kernel_store),
            Arc::clone(&scheduler),
            aura_engine::AgentHostConfig::default(),
        );
        (host, scheduler)
    });
    let traces = event_loop::Traces::new(
        trace_home,
        Some(Arc::new(aura_store_db::TraceStore::for_store(&store))),
        record_router,
        commands.clone(),
    );
    if let Some((host, _)) = &host {
        host.set_turn_observer(Arc::clone(&traces) as Arc<dyn aura_engine::TurnObserver>);
    }
    let processes = event_loop::ProcessTable::new(
        host.as_ref().map(|(host, _)| host.clone()),
        Some(Arc::clone(&process_manager)),
    );
    let background: Arc<dyn BackgroundProcessHook> = Arc::new(event_loop::BackgroundCommands::new(
        process_manager,
        store.clone() as Arc<dyn aura_store_db::ReadStore>,
        data_dir.join("processes"),
        &processes,
    ));
    let agents = host.map(|(host, scheduler)| {
        event_loop::TerminalAgents::new(
            host,
            scheduler,
            Arc::new(
                event_loop::TerminalChildKernels::new(tool_config.clone(), Arc::clone(&background))
                    .with_command_output(commands.clone()),
            ),
        )
    });
    let kernel_agents = agents.clone();
    let kernel_background = Arc::clone(&background);

    let checkpoints = crate::session_checkpoint::Checkpoints::new(&data_dir);
    let file_checkpoints = crate::file_checkpoints::FileCheckpoints::new(&data_dir);
    let factory_files = file_checkpoints.clone();
    let diff_commands = commands.clone();
    let check_commands = commands.clone();
    let hub = event_loop::AgentHub::new(
        event_loop::AgentHubParts {
            registry,
            definitions,
            skills: SkillManager::new(SkillLoader::with_defaults(Some(workspace_root), None)),
            base,
            commands,
            long_run,
            mode: permissions.mode,
            approval: permissions.approval,
            approvals: approvals.clone(),
            instructions,
            processes,
            agents,
            record_panel,
            gate,
            traces,
            channels: Some(aura_store_db::ChannelStore::for_store(&store)),
        },
        move |dispatcher| {
            event_loop::KernelFactory::new(
                store,
                Box::new(move |agent_id, parent_chain, folder| {
                    let chain = std::iter::once(agent_id)
                        .chain(parent_chain.iter().copied())
                        .collect();
                    let file_hook = factory_files.hook(chain, Some(diff_commands.clone()));
                    let command_output =
                        event_loop::TerminalCommandOutput::new(diff_commands.clone(), agent_id);
                    let mut executor = ToolExecutor::new(tool_config.clone())
                        .with_caller_permissions(AgentPermissions::full_access())
                        .with_parent_chain(parent_chain)
                        .with_subagent_dispatch_hook(Arc::clone(&dispatcher))
                        .with_background_process_hook(Arc::clone(&kernel_background))
                        .with_file_change_hook(file_hook)
                        .with_command_output_hook(Arc::new(command_output));
                    if let Some(agents) = &kernel_agents {
                        let hooks = agents.hooks_for(agent_id);
                        executor = executor
                            .with_spawn_hook(hooks.clone())
                            .with_agent_control_hook(hooks.clone())
                            .with_agent_read_hook(hooks);
                    }
                    executor.register(Box::new(TodoWriteTool));
                    web_tools.register(&mut executor);
                    let mut executor_router = ExecutorRouter::new();
                    executor_router.add_executor(Arc::new(executor));
                    let mut config = kernel_config.clone();
                    if let Some(folder) = folder {
                        config.workspace_base = folder;
                    }
                    if let Some(approvals) = &approvals {
                        approvals.set_folder(agent_id, config.workspace_base.clone());
                    }
                    Ok(Kernel::new(
                        Arc::clone(&kernel_store),
                        Arc::clone(&provider),
                        executor_router,
                        config,
                        agent_id,
                    )?)
                }),
            )
            .with_checkpoints(checkpoints)
            .with_file_checkpoints(file_checkpoints)
            .with_post_edit_check(post_edit_check, check_commands)
        },
    );
    Harness { hub, background }
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
