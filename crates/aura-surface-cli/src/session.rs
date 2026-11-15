//! Agent session behind an interactive UI.
//!
//! [`start`] opens the agent registry, record store, model provider and
//! kernel factory for the current workspace and spawns the event loop.
//! The UI on top only sees the channel pair in [`UiSession`]: it sends
//! [`UiEvent`]s and applies the [`UiCommand`]s it receives. The ratatui
//! terminal and the native desktop app (`aura-desktop`) both run on it.

use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::cli::RunArgs;
use crate::model_selection::ModelSelection;
use crate::{api_server, event_loop, record_loader, session_helpers};

use anyhow::Context;
use aura_agent::{AgentLoop, ProcessManager, ProcessManagerConfig};
use aura_agent_kernel::{ExecutorRouter, Kernel, KernelConfig};
use aura_context_skills::{SkillLoader, SkillManager};
use aura_core_types::{AgentPermissions, Transaction};
use aura_model_reasoner::ModelProvider;
use aura_terminal::{UiCommand, UiEvent};
use aura_tools::{ToolConfig, ToolExecutor};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// How long a background command may run before it is killed. Long
/// enough for a dev server to outlive a working day.
const BACKGROUND_COMMAND_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000;

/// A running agent session, as seen by the UI driving it.
pub struct UiSession {
    /// Where the UI sends user actions
    pub events: mpsc::Sender<UiEvent>,
    /// State updates for the UI, in order
    pub commands: mpsc::Receiver<UiCommand>,
    /// The event loop serving `events`
    pub task: SessionTask,
}

/// The spawned agent event loop of a [`UiSession`].
pub struct SessionTask(JoinHandle<anyhow::Result<()>>);

impl SessionTask {
    /// Call once the UI has quit. Surfaces the error when the event loop
    /// already stopped on its own (so the process exits non-zero), and
    /// aborts it when it is still running.
    ///
    /// # Errors
    ///
    /// The event loop's error, or a join error when it panicked.
    pub async fn finish(self) -> anyhow::Result<()> {
        if !self.0.is_finished() {
            self.0.abort();
            return Ok(());
        }
        match self.0.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e.context("agent event loop failed")),
            Err(join_err) if join_err.is_cancelled() => Ok(()),
            Err(join_err) => Err(anyhow::anyhow!(
                "agent event loop task did not join cleanly: {join_err}"
            )),
        }
    }
}

/// Start an agent session in the current working directory.
///
/// # Errors
///
/// Fails when the data directory, agent registry, record store, sign-in
/// state or model provider can't be opened.
pub async fn start(
    args: &RunArgs,
    requested_mode: Option<aura_core_modes::AgentMode>,
) -> anyhow::Result<UiSession> {
    let (ui_tx, mut ui_rx) = mpsc::channel::<UiEvent>(100);
    let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>(200);

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

    let mut tool_config = ToolConfig::for_autonomous_dev_loop();
    if args.allow_unrestricted_full_access || unrestricted_full_access_from_env() {
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
    let (_, tools) = session_helpers::build_executor_router_with_config(&tool_config);

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
        ProcessManagerConfig {
            max_async_timeout_ms: BACKGROUND_COMMAND_TIMEOUT_MS,
            ..ProcessManagerConfig::default()
        },
    ));
    let background_commands: Arc<dyn aura_tools::BackgroundProcessHook> =
        Arc::new(event_loop::BackgroundCommands::new(
            Arc::clone(&process_manager),
            store.clone() as Arc<dyn aura_store_db::ReadStore>,
            data_dir.join("processes"),
        ));

    // `mock` for explicit test / offline mode; otherwise the routed
    // provider sends `local/*` models to the local server (Ollama),
    // `claude/*` and `chatgpt/*` to the linked plan, and everything else
    // to aura-router.
    let resolver: Arc<dyn aura_auth::CredentialResolver> = auth.clone();
    let selection = match args.provider.as_str() {
        "mock" => Ok(aura_model_reasoner::mock_provider()),
        _ => aura_model_reasoner::routed_provider(Some(resolver)),
    }
    .context("building model provider")?;
    if selection.name == "mock" {
        let _ = cmd_tx.try_send(UiCommand::SetStatus("Mock Mode".to_string()));
    }
    let provider: Arc<dyn ModelProvider + Send + Sync> = selection.provider;

    let record_panel = event_loop::RecordPanelAgent::default();
    let record_observer: Arc<dyn aura_agent_kernel::RecordObserver> = Arc::new(
        event_loop::UiRecordObserver::new(cmd_tx.clone(), record_panel.clone()),
    );
    let kernel_config = KernelConfig {
        workspace_base: workspace_root.clone(),
        use_workspace_base_as_root: true,
        policy: aura_agent_kernel::PolicyConfig::default()
            .with_mode(aura_config::session_mode(requested_mode)),
        record_observer: Some(Arc::clone(&record_observer)),
        ..KernelConfig::default()
    };
    let kernel_store = store.clone() as Arc<dyn aura_store_db::Store>;
    let scheduler = Arc::new(
        aura_engine::Scheduler::new(
            Arc::clone(&kernel_store),
            Arc::clone(&provider),
            Vec::new(),
            tools.clone(),
            workspace_root.clone(),
            None,
        )
        .with_record_observer(record_observer),
    );
    let host = aura_engine::AgentHost::new(
        Arc::clone(&kernel_store),
        scheduler.clone(),
        aura_engine::AgentHostConfig::default(),
    );
    let agents = event_loop::TerminalAgents::new(
        host.clone(),
        scheduler,
        Arc::new(event_loop::TerminalChildKernels::new(
            tool_config.clone(),
            Arc::clone(&background_commands),
        )),
        aura_config::session_mode(requested_mode),
    );
    let processes = event_loop::ProcessTable::new(Some(host), Some(Arc::clone(&process_manager)));
    let kernel_agents = agents.clone();
    let hub = event_loop::AgentHub::new(
        event_loop::AgentHubParts {
            registry,
            definitions,
            skills: SkillManager::new(SkillLoader::with_defaults(
                Some(workspace_root.clone()),
                None,
            )),
            base: event_loop::TurnBase {
                config: base_config,
                tools,
                limits: models.limits(),
            },
            commands: cmd_tx.clone(),
            processes: Arc::clone(&processes),
            agents: Some(agents),
            record_panel: record_panel.clone(),
        },
        move |dispatcher| {
            event_loop::KernelFactory::new(
                store,
                Box::new(move |agent_id, parent_chain, folder| {
                    let hooks = kernel_agents.hooks_for(agent_id);
                    let executor = ToolExecutor::new(tool_config.clone())
                        .with_spawn_hook(hooks.clone())
                        .with_agent_control_hook(hooks.clone())
                        .with_agent_read_hook(hooks)
                        .with_caller_permissions(AgentPermissions::full_access())
                        .with_parent_chain(parent_chain)
                        .with_subagent_dispatch_hook(Arc::clone(&dispatcher))
                        .with_background_process_hook(Arc::clone(&background_commands));
                    let mut executor_router = ExecutorRouter::new();
                    executor_router.add_executor(Arc::new(executor));
                    let mut config = kernel_config.clone();
                    if let Some(folder) = folder {
                        config.workspace_base = folder;
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
        },
    );
    let _ = cmd_tx.try_send(UiCommand::SetAgents(hub.agent_summaries()));
    let _ = cmd_tx.try_send(UiCommand::SetActiveAgent(identity.agent_id.to_hex()));
    let _ = cmd_tx.try_send(UiCommand::SetAgentDefinitions(hub.definition_summaries()));
    let session = hub.factory.open(&active_agent, Vec::new(), active_folder)?;
    record_panel.set(session.agent_id());

    let cmd_tx_clone = cmd_tx.clone();

    let processor_handle = tokio::spawn(async move {
        let mut agent_loop = agent_loop;
        let ctx = event_loop::EventLoopContext {
            events: &mut ui_rx,
            process_completions: process_rx,
            commands: cmd_tx_clone,
            agent_loop: &mut agent_loop,
            session,
            hub,
            processes,
            memory_manager: None,
            models,
            auth,
        };
        event_loop::run_event_loop(ctx).await
    });

    Ok(UiSession {
        events: ui_tx,
        commands: cmd_rx,
        task: SessionTask(processor_handle),
    })
}

fn unrestricted_full_access_from_env() -> bool {
    std::env::var("AURA_ALLOW_UNRESTRICTED_FULL_ACCESS").is_ok_and(|value| {
        let value = value.trim();
        value == "1" || value.eq_ignore_ascii_case("true")
    })
}
