//! Phase 10 carve-out 1: lifted body of the `aura` CLI binary. The
//! root `src/main.rs` is reduced to a thin entrypoint that initialises
//! `dotenvy` and calls [`run`]; every line of useful CLI logic now
//! lives in this module. The `aura-node` entrypoint lives in
//! `aura_runtime::run_node` and is re-exported from this crate root.

use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::cli::{Cli, Commands, MigrateArgs, PluginsCommand, PluginsSubcommand, RunArgs, UiMode};
use crate::long_run::{add_long_run_tools, WebTools};
use crate::project_instructions::{InstructionSettings, ProjectInstructions};
use crate::{auth_commands, event_loop, session, session_helpers};

use anyhow::Context;
use aura_agent::{ProcessManager, ProcessManagerConfig};
use aura_agent_kernel::{
    ExecutorRouter, Kernel, KernelConfig, RecordObserver, ToolApprovalPrompter,
};
use aura_context_skills::{SkillLoader, SkillManager};
use aura_core_types::{AgentPermissions, Transaction};
use aura_model_reasoner::ModelProvider;
use aura_terminal::{AlertSettings, App, PermissionMode, Terminal, Theme, UiCommand};
use aura_tools::todo::TodoWriteTool;
use aura_tools::{BackgroundProcessHook, ToolConfig, ToolExecutor};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// How long a background command may run before it is killed. Long
/// enough for a dev server to outlive a working day.
const BACKGROUND_COMMAND_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000;

// ============================================================================
// Hello World (Spec 01)
// ============================================================================

/// Phase 10 surface-layer wrapper for the Spec 01 hello-world
/// banner. Kept public so external smoke tests can pin the
/// canonical message without depending on the binary crate.
#[must_use]
pub fn hello_world_message() -> &'static str {
    "Hello, World!"
}

/// Print the hello-world banner to stdout (Spec 01).
pub fn hello_world() {
    println!("{}", hello_world_message());
}

// ============================================================================
// Entry points
// ============================================================================

/// Surface-layer entry point for the `aura` binary.
///
/// Parses CLI arguments and dispatches to the appropriate
/// subcommand. The root `src/main.rs` is reduced to a thin shim
/// that calls `aura_surface_cli::run().await` after invoking
/// `dotenvy::dotenv()`.
///
/// # Errors
///
/// Surfaces any error from the dispatched subcommand. Subcommand
/// errors are typed at their source (e.g. `aura_auth::AuthError`,
/// `aura_plugin_core::InstallError`) and bubble up here as
/// `anyhow::Error` because the root binary's `main` is the only
/// remaining `anyhow`-using boundary in the workspace.
pub async fn run() -> anyhow::Result<std::process::ExitCode> {
    let cli = Cli::parse();
    let requested_mode = cli.mode.map(aura_core_modes::AgentMode::from);

    if let Some(prompt) = cli.print {
        if cli.command.is_some() {
            anyhow::bail!("-p runs a prompt on its own; it cannot be combined with a subcommand");
        }
        return crate::headless::run(prompt, cli.headless, requested_mode).await;
    }

    match cli.command {
        Some(Commands::Hello) => {
            hello_world();
            Ok(())
        }
        Some(Commands::Login(args)) => auth_commands::cmd_login(args).await,
        Some(Commands::Logout(args)) => auth_commands::cmd_logout(args).await,
        Some(Commands::Whoami) => auth_commands::cmd_whoami().await,
        Some(Commands::Auth(args)) => auth_commands::cmd_auth(args),
        Some(Commands::Migrate(args)) => cmd_migrate(args),
        Some(Commands::Plugins(args)) => cmd_plugins(args),
        Some(Commands::Run(args)) => run_with_args(args, requested_mode).await,
        None => run_with_args(RunArgs::default(), requested_mode).await,
    }
    .map(|()| std::process::ExitCode::SUCCESS)
}

// ============================================================================
// Subcommand handlers
// ============================================================================

/// `aura migrate` (Phase 4a stub).
///
/// Intentionally a no-op today. Future phases will populate this
/// with the Codex → Aura state migration (config, credentials,
/// session history). Documented in the plan
/// (`phase-4a-config-aura-home`) so the CLI surface is stable when
/// later phases land the real migration logic.
fn cmd_migrate(args: MigrateArgs) -> anyhow::Result<()> {
    eprintln!(
        "aura migrate stub - Phase 4a placeholder; no migration actions performed (dry_run={})",
        args.dry_run
    );
    Ok(())
}

/// `aura plugins` (Phase 4b).
///
/// Dispatches to the install / list / enable / disable handlers.
/// Resolves `AURA_HOME` once and constructs a single
/// [`aura_plugin_core::PluginCache`] rooted at `AURA_HOME/plugins/`
/// before dispatching so every subcommand sees the same cache root
/// resolution rules.
fn cmd_plugins(args: PluginsCommand) -> anyhow::Result<()> {
    let aura_home = aura_config::AuraHome::resolve().context("resolving AURA_HOME")?;
    let cache = aura_plugin_core::PluginCache::new(aura_home.path.join("plugins"));
    match args.action {
        PluginsSubcommand::Install { source, trust } => {
            let installed = aura_plugin_core::install_with_trust(&source, &cache, trust)
                .with_context(|| format!("installing plugin from {}", source.display()))?;
            println!(
                "installed {} v{}",
                installed.manifest.id.as_str(),
                installed.manifest.version
            );
        }
        PluginsSubcommand::List => {
            let ids = cache.list_plugins().context("listing plugin cache")?;
            if ids.is_empty() {
                println!("(no plugins installed under {})", cache.root().display());
            }
            for id in ids {
                let active = cache
                    .active_version(&id)
                    .with_context(|| format!("reading active pointer for {id}"))?
                    .unwrap_or_else(|| "(none active)".to_string());
                println!("{id:<32} active={active}");
            }
        }
        PluginsSubcommand::Enable { id, yes, no } => {
            cmd_plugin_enable(&aura_home.path, &cache, &id, yes, no)?;
        }
        PluginsSubcommand::Disable { id } => {
            update_plugin_enable(&aura_home.path, &id, false, None)?;
            println!("disabled {id}");
        }
    }
    Ok(())
}

/// `aura plugins enable` flow with trust-prompt support.
///
/// Reads the prior `[plugins.<id>]` state from `config.toml`, hands
/// the cache + state to [`aura_plugin_core::enable_with_prompter`]
/// with the appropriate prompter (`AlwaysYes` for `--yes`,
/// `AlwaysNo` for `--no`, `TtyPrompter` otherwise), and writes the
/// resulting decision back to `config.toml`.
fn cmd_plugin_enable(
    aura_home: &std::path::Path,
    cache: &aura_plugin_core::PluginCache,
    id: &str,
    yes: bool,
    no: bool,
) -> anyhow::Result<()> {
    use aura_plugin_core::{
        enable_with_prompter, AlwaysNo, AlwaysYes, EnableDecision, TtyPrompter,
    };

    let prior = read_plugin_enable_state(aura_home, id)?;

    let outcome = if yes {
        enable_with_prompter(cache, id, prior, &mut AlwaysYes)
    } else if no {
        enable_with_prompter(cache, id, prior, &mut AlwaysNo)
    } else {
        enable_with_prompter(cache, id, prior, &mut TtyPrompter)
    }
    .with_context(|| format!("enabling plugin `{id}`"))?;

    match outcome.decision {
        EnableDecision::Enabled => {
            update_plugin_enable(
                aura_home,
                id,
                outcome.enabled_after,
                Some(outcome.trusted_after),
            )?;
            println!("enabled {id} (v{})", outcome.version);
        }
        EnableDecision::AlreadyTrusted => {
            update_plugin_enable(aura_home, id, true, Some(true))?;
            println!("enabled {id} (v{}; already trusted)", outcome.version);
        }
        EnableDecision::TrustDeclined => {
            println!("trust declined; plugin not enabled");
        }
    }
    Ok(())
}

/// Read `[plugins.<id>]` from `<aura_home>/config.toml`, returning a
/// snapshot of the `enabled` / `trusted` booleans. Returns the empty
/// state when the file or section is missing.
fn read_plugin_enable_state(
    aura_home: &std::path::Path,
    id: &str,
) -> anyhow::Result<aura_plugin_core::PluginEnableState> {
    use std::fs;

    let config_path = aura_home.join("config.toml");
    if !config_path.exists() {
        return Ok(aura_plugin_core::PluginEnableState::default());
    }
    let body = fs::read_to_string(&config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let root: toml::Value =
        toml::from_str(&body).with_context(|| format!("parsing {}", config_path.display()))?;
    let table = root
        .get("plugins")
        .and_then(|p| p.get(id))
        .and_then(|p| p.as_table());
    let enabled = table
        .and_then(|t| t.get("enabled"))
        .and_then(toml::Value::as_bool);
    let trusted = table
        .and_then(|t| t.get("trusted"))
        .and_then(toml::Value::as_bool);
    Ok(aura_plugin_core::PluginEnableState { enabled, trusted })
}

/// Minimal `~/.aura/config.toml` mutation: read (or synthesise) the
/// file, set `plugins.<id>.enabled = <enabled>`, atomic-write back
/// via tempfile + rename.
///
/// We intentionally use `toml::Value` (already a workspace dep) rather
/// than pulling in `toml_edit` for Phase 4b. The trade-off is that
/// comment / formatting is not preserved — acceptable today because
/// the config file is operator-owned and the round-trip writes a
/// consistent canonical layout.
fn update_plugin_enable(
    aura_home: &std::path::Path,
    id: &str,
    enabled: bool,
    trusted: Option<bool>,
) -> anyhow::Result<()> {
    use std::fs;

    let config_path = aura_home.join("config.toml");
    fs::create_dir_all(aura_home)
        .with_context(|| format!("creating AURA_HOME at {}", aura_home.display()))?;

    let mut root: toml::Value = if config_path.exists() {
        let body = fs::read_to_string(&config_path)
            .with_context(|| format!("reading {}", config_path.display()))?;
        toml::from_str(&body).with_context(|| format!("parsing {}", config_path.display()))?
    } else {
        toml::Value::Table(toml::value::Table::new())
    };

    let root_table = root
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config.toml root must be a table"))?;

    let plugins_entry = root_table
        .entry("plugins".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    let plugins_table = plugins_entry
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("[plugins] entry must be a table"))?;

    let plugin_entry = plugins_table
        .entry(id.to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    let plugin_table = plugin_entry
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("[plugins.{id}] entry must be a table"))?;

    plugin_table.insert("enabled".to_string(), toml::Value::Boolean(enabled));
    if let Some(t) = trusted {
        plugin_table.insert("trusted".to_string(), toml::Value::Boolean(t));
    }

    let serialised = toml::to_string_pretty(&root).context("serialising updated config.toml")?;

    let tmp = config_path.with_extension("toml.tmp");
    fs::write(&tmp, serialised).with_context(|| format!("writing {}", tmp.display()))?;
    if config_path.exists() {
        // Windows fs::rename refuses to overwrite; remove first.
        let _ = fs::remove_file(&config_path);
    }
    fs::rename(&tmp, &config_path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), config_path.display()))?;
    Ok(())
}

async fn run_with_args(
    args: RunArgs,
    requested_mode: Option<aura_core_modes::AgentMode>,
) -> anyhow::Result<()> {
    match args.ui {
        UiMode::Terminal => run_terminal(args, requested_mode).await,
        UiMode::None => {
            let filter = if args.verbose {
                EnvFilter::from_default_env().add_directive("aura=debug".parse()?)
            } else {
                EnvFilter::from_default_env().add_directive("aura=info".parse()?)
            };

            tracing_subscriber::registry()
                .with(
                    fmt::layer()
                        .event_format(aura_runtime::console_format::AuraConsoleFormat::new()),
                )
                .with(filter)
                .init();

            if let Some(mode) = requested_mode {
                warn!(
                    ?mode,
                    "--mode does not apply to the headless node; set the per-run `mode` \
                     field on POST /v1/run or AURA_DEFAULT_MODE instead"
                );
            }
            run_headless(args).await
        }
    }
}

// ============================================================================
// Terminal Mode
// ============================================================================

async fn run_terminal(
    args: RunArgs,
    requested_mode: Option<aura_core_modes::AgentMode>,
) -> anyhow::Result<()> {
    let theme = Theme::by_name(&args.theme);
    let session = session::start(&args, requested_mode).await?;

    let mut app = App::new()
        .with_event_sender(session.events.clone())
        .with_command_receiver(session.commands)
        .with_alert_settings(AlertSettings::from_env());
    if args.verbose {
        app.set_verbose(true);
    }

    let mut terminal = Terminal::new(theme)?;
    let ran = terminal.run(&mut app);
    let finished = session.task.finish().await;
    ran?;
    finished
}

// ============================================================================
// Agent harness shared by the terminal and `aura -p`
// ============================================================================

/// Tool settings, the model-facing tool list and the web tools for
/// terminal-style agents: the autonomous dev-loop tools plus planning,
/// background jobs, `web_fetch` and `web_search`. The web tools must be
/// registered on each agent's executor (see [`build_hub`]) to match the
/// list.
pub(crate) fn agent_tools(
    allow_unrestricted_full_access: bool,
) -> (
    ToolConfig,
    Vec<aura_model_reasoner::ToolDefinition>,
    WebTools,
) {
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
) -> anyhow::Result<aura_model_reasoner::ProviderSelection> {
    let resolver: Arc<dyn aura_auth::CredentialResolver> = auth.clone();
    match name {
        "mock" => Ok(aura_model_reasoner::mock_provider()),
        _ => aura_model_reasoner::routed_provider(Some(resolver)),
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
    /// Kernel policy for every agent: mode and tool permissions
    pub policy: aura_agent_kernel::PolicyConfig,
    /// Sees every record entry of every agent, including background ones
    pub record_observer: Option<Arc<dyn RecordObserver>>,
    /// Which agent is on screen; shared with `record_observer`
    pub record_panel: event_loop::RecordPanelAgent,
    /// Where agents' chats and panels go
    pub commands: mpsc::Sender<UiCommand>,
    pub long_run: crate::long_run::LongRunSettings,
    /// Runs `run_command`'s background commands and reports their exits;
    /// from [`background_process_manager`]
    pub process_manager: Arc<ProcessManager>,
    /// Offer `spawn_agent` and the tools that talk to background agents
    pub background_agents: bool,
    /// Permission mode agents start in; spawned agents take their
    /// parent's. Should match the mode `policy` was built for.
    pub mode: PermissionMode,
    pub instructions: ProjectInstructions,
    /// Asks the user about tool calls the policy wants approved. Without
    /// it the kernel denies those calls, which `aura -p` relies on.
    pub approvals: Option<Arc<event_loop::TerminalApprovals>>,
    /// Registered on every agent's executor; from [`agent_tools`]
    pub web_tools: WebTools,
    /// Run after each step that edits files; results go to `commands`
    pub post_edit_check: aura_agent::PostEditCheckConfig,
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
pub(crate) fn policy_tool_names(tools: &[aura_model_reasoner::ToolDefinition]) -> Vec<String> {
    let mut names: Vec<String> = tools.iter().map(|tool| tool.name.clone()).collect();
    names.push(aura_tools::agents::task::TASK_TOOL_NAME.to_string());
    names
}

/// Kernel policy for agents in `mode`, the same table the hub applies
/// when an agent switches mode or a spawn takes its parent's.
pub(crate) fn mode_policy(
    mode: PermissionMode,
    tool_names: &[String],
) -> aura_agent_kernel::PolicyConfig {
    aura_agent_kernel::PolicyConfig::default()
        .with_mode(mode.agent_mode())
        .with_user_default(event_loop::modes::user_defaults(mode, tool_names))
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
        policy,
        record_observer,
        record_panel,
        commands,
        long_run,
        process_manager,
        background_agents,
        mode,
        instructions,
        approvals,
        web_tools,
        post_edit_check,
    } = setup;
    let kernel_config = KernelConfig {
        workspace_base: workspace_root.clone(),
        use_workspace_base_as_root: true,
        policy,
        tool_approval_prompter: approvals
            .clone()
            .map(|approvals| approvals as Arc<dyn ToolApprovalPrompter>),
        record_observer: record_observer.clone(),
        ..KernelConfig::default()
    };
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
        let scheduler = Arc::new(scheduler);
        let host = aura_engine::AgentHost::new(
            Arc::clone(&kernel_store),
            Arc::clone(&scheduler),
            aura_engine::AgentHostConfig::default(),
        );
        (host, scheduler)
    });
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
            Arc::new(event_loop::TerminalChildKernels::new(
                tool_config.clone(),
                Arc::clone(&background),
            )),
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
            mode,
            instructions,
            processes,
            agents,
            record_panel,
        },
        move |dispatcher| {
            event_loop::KernelFactory::new(
                store,
                Box::new(move |agent_id, parent_chain, folder| {
                    let chain = std::iter::once(agent_id)
                        .chain(parent_chain.iter().copied())
                        .collect();
                    let file_hook = factory_files.hook(chain, Some(diff_commands.clone()));
                    let mut executor = ToolExecutor::new(tool_config.clone())
                        .with_caller_permissions(AgentPermissions::full_access())
                        .with_parent_chain(parent_chain)
                        .with_subagent_dispatch_hook(Arc::clone(&dispatcher))
                        .with_background_process_hook(Arc::clone(&kernel_background))
                        .with_file_change_hook(file_hook);
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

// ============================================================================
// Headless Mode (Node)
// ============================================================================

async fn run_headless(args: RunArgs) -> anyhow::Result<()> {
    info!("Starting AURA CLI in headless mode (node server)");

    let mut config = aura_runtime::NodeConfig::from_env();
    config.allow_unrestricted_full_access |= args.allow_unrestricted_full_access;

    aura_runtime::Node::new(config).run().await
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

fn unrestricted_full_access_from_env() -> bool {
    std::env::var("AURA_ALLOW_UNRESTRICTED_FULL_ACCESS").is_ok_and(|value| {
        let value = value.trim();
        value == "1" || value.eq_ignore_ascii_case("true")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hello_world_message_exact() {
        assert_eq!(hello_world_message(), "Hello, World!");
    }

    #[test]
    fn test_hello_world_prints() {
        hello_world();
    }
}
