//! Phase 10 carve-out 1: lifted body of the `aura` CLI binary. The
//! root `src/main.rs` is reduced to a thin entrypoint that initialises
//! `dotenvy` and calls [`run`]; every line of useful CLI logic now
//! lives in this module. The `aura-node` entrypoint lives in
//! `aura_runtime::run_node` and is re-exported from this crate root.

use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::cli::{Cli, Commands, MigrateArgs, PluginsCommand, PluginsSubcommand, RunArgs, UiMode};
use crate::long_run::{add_long_run_tools, WebTools};
use crate::model_selection::ModelSelection;
use crate::project_instructions::{InstructionSettings, ProjectInstructions};
use crate::{api_server, auth_commands, event_loop, record_loader, session_helpers};

use anyhow::Context;
use aura_agent::{AgentLoop, ProcessManager, ProcessManagerConfig};
use aura_agent_kernel::{ExecutorRouter, Kernel, KernelConfig, ToolApprovalPrompter};
use aura_context_skills::{SkillLoader, SkillManager};
use aura_core_types::{AgentPermissions, Transaction};
use aura_model_reasoner::ModelProvider;
use aura_terminal::{AlertSettings, App, PermissionMode, Terminal, Theme, UiCommand, UiEvent};
use aura_tools::jobs::{BackgroundJobs, JobFinished};
use aura_tools::todo::TodoWriteTool;
use aura_tools::{ToolConfig, ToolExecutor};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

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

    let (ui_tx, mut ui_rx) = mpsc::channel::<UiEvent>(100);
    let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>(200);

    let mut app = App::new()
        .with_event_sender(ui_tx.clone())
        .with_command_receiver(cmd_rx)
        .with_alert_settings(AlertSettings::from_env());

    if args.verbose {
        app.set_verbose(true);
    }

    let mut terminal = Terminal::new(theme)?;

    let data_dir = args
        .dir
        .clone()
        .or_else(|| std::env::var("AURA_DATA_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("./aura_data"));

    // Tools run in the directory the terminal was launched from, so typed
    // commands and model tool calls see the user's project rather than an
    // empty per-agent scratch dir under `data_dir`.
    let workspace_root = std::env::current_dir().context("resolving working directory")?;
    tokio::fs::create_dir_all(&data_dir).await?;

    let registry = AgentRegistry::load_or_init(&data_dir).context("loading agent registry")?;
    let active_agent = registry.active().clone();
    let identity = active_agent.identity();
    let active_folder = registry.project_folder(identity.agent_id);
    let definitions =
        AgentDefinitions::load_or_seed(&data_dir).context("loading permanent agents")?;

    let store_path = session_helpers::resolve_store_path(&data_dir);
    let store = session_helpers::open_store(&store_path)?;

    record_loader::load_existing_records(store.as_ref(), identity.agent_id, &cmd_tx);
    match record_loader::conversation_history(store.as_ref(), identity.agent_id) {
        Ok(history) => {
            let _ = cmd_tx.try_send(UiCommand::LoadConversation(history));
        }
        Err(e) => warn!(error = %e, "Failed to rebuild conversation from record"),
    }
    api_server::start_api_server(cmd_tx.clone(), workspace_root.clone()).await;

    let (tool_config, tools, web_tools) = agent_tools(args.allow_unrestricted_full_access);
    let (job_tx, job_rx) = mpsc::channel::<JobFinished>(64);
    let jobs = BackgroundJobs::new(job_tx);

    // The terminal harness is the one surface that resolves its model
    // locally (env, persisted `/model` choice, then the local default) —
    // higher-level surfaces (chat WS, dev-loop, task-run) plumb the
    // user-selected model through their own paths.
    let mut config = session_helpers::default_agent_config(aura_model_reasoner::ENV_FALLBACK_MODEL);
    let auth = Arc::new(aura_auth::AuthManager::new().context("loading sign-in state")?);
    let summary = event_loop::auth::auth_summary(&auth, config.auth_token.as_deref());
    let models = ModelSelection::init(&mut config, summary);
    info!(model = %models.current(), "Terminal model selected");
    let base_config = config.clone();
    let agent_loop = AgentLoop::new(config);

    let (process_tx, process_rx) = mpsc::channel::<Transaction>(100);
    let process_manager = Arc::new(ProcessManager::new(
        process_tx,
        ProcessManagerConfig::default(),
    ));

    // `mock` for explicit test / offline mode; otherwise the routed
    // provider sends `local/*` models to the local server (Ollama),
    // `claude/*` and `chatgpt/*` to the linked plan, and everything else
    // to aura-router.
    let selection = build_provider(&args.provider, &auth)?;
    if selection.name == "mock" {
        let _ = cmd_tx.try_send(UiCommand::SetStatus("Mock Mode".to_string()));
    }

    let mode = initial_permission_mode(requested_mode);
    info!(%mode, "Terminal permission mode");
    let tool_names = policy_tool_names(&tools);
    let approvals = Arc::new(event_loop::TerminalApprovals::new(
        cmd_tx.clone(),
        &data_dir,
    ));
    let hub = build_hub(HubSetup {
        data_dir: data_dir.clone(),
        workspace_root: workspace_root.clone(),
        registry,
        definitions,
        store,
        provider: selection.provider,
        tool_config,
        base: event_loop::TurnBase {
            config: base_config,
            tools,
            limits: models.limits(),
        },
        policy: mode_policy(mode, &tool_names),
        tool_audit_observer: Some(Arc::new(event_loop::UiToolAuditObserver::new(
            cmd_tx.clone(),
        ))),
        commands: cmd_tx.clone(),
        long_run: crate::long_run::LongRunSettings::from_env(),
        jobs: Arc::clone(&jobs),
        mode,
        instructions: project_instructions(),
        approvals: Some(Arc::clone(&approvals)),
        web_tools,
        post_edit_check: post_edit_check_from_env(),
    });
    let _ = cmd_tx.try_send(UiCommand::SetAgents(hub.agent_summaries()));
    let _ = cmd_tx.try_send(UiCommand::SetActiveAgent(identity.agent_id.to_hex()));
    let _ = cmd_tx.try_send(UiCommand::SetAgentDefinitions(hub.definition_summaries()));
    let session = hub.factory.open(&active_agent, Vec::new(), active_folder)?;

    let cmd_tx_clone = cmd_tx.clone();
    let process_manager_clone = Arc::clone(&process_manager);
    let loop_jobs = Arc::clone(&jobs);

    let processor_handle = tokio::spawn(async move {
        let mut agent_loop = agent_loop;
        let ctx = event_loop::EventLoopContext {
            events: &mut ui_rx,
            process_completions: process_rx,
            commands: cmd_tx_clone,
            agent_loop: &mut agent_loop,
            session,
            hub,
            _process_manager: process_manager_clone,
            memory_manager: None,
            models,
            auth,
            job_finished: job_rx,
            jobs: loop_jobs,
            approvals,
        };
        event_loop::run_event_loop(ctx).await
    });

    let ran = terminal.run(&mut app);
    // Jobs lead their own process groups, so nothing else stops them
    // when the terminal exits.
    jobs.kill_all();
    ran?;

    // Phase 5 (error-handling polish): we used to call
    // `processor_handle.abort()` and return `Ok(())` unconditionally,
    // which masked any panic / structured error inside the agent
    // loop. Surface the failure when the loop has already terminated
    // (so the CLI exits non-zero), and only fall back to `abort` for
    // the ordinary case where the user quit the UI while the loop
    // was still running.
    if processor_handle.is_finished() {
        match processor_handle.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e.context("agent event loop failed")),
            Err(join_err) if join_err.is_cancelled() => {}
            Err(join_err) => {
                return Err(anyhow::anyhow!(
                    "agent event loop task did not join cleanly: {join_err}"
                ));
            }
        }
    } else {
        processor_handle.abort();
    }

    Ok(())
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
    pub tool_audit_observer: Option<Arc<dyn aura_agent_kernel::ToolAuditObserver>>,
    /// Where agents' chats and panels go
    pub commands: mpsc::Sender<UiCommand>,
    pub long_run: crate::long_run::LongRunSettings,
    pub jobs: Arc<BackgroundJobs>,
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

/// The agent hub: registry, permanent agents, and a kernel factory whose
/// kernels run the tools in `setup` under its policy and checkpoint their
/// sessions under `data_dir`.
pub(crate) fn build_hub(setup: HubSetup) -> Arc<event_loop::AgentHub> {
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
        tool_audit_observer,
        commands,
        long_run,
        jobs,
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
        tool_audit_observer,
        ..KernelConfig::default()
    };
    let kernel_store = store.clone() as Arc<dyn aura_store_db::Store>;
    let checkpoints = crate::session_checkpoint::Checkpoints::new(&data_dir);
    let file_checkpoints = crate::file_checkpoints::FileCheckpoints::new(&data_dir);
    let factory_files = file_checkpoints.clone();
    let diff_commands = commands.clone();
    let check_commands = commands.clone();
    event_loop::AgentHub::new(
        event_loop::AgentHubParts {
            registry,
            definitions,
            skills: SkillManager::new(SkillLoader::with_defaults(Some(workspace_root), None)),
            base,
            commands,
            long_run,
            mode,
            instructions,
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
                        .with_background_jobs(Arc::clone(&jobs))
                        .with_file_change_hook(file_hook);
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
    )
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
fn initial_permission_mode(requested: Option<aura_core_modes::AgentMode>) -> PermissionMode {
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
