//! Agent session behind an interactive UI.
//!
//! [`start`] opens the agent registry, record store, model provider and
//! kernel factory for the current workspace and spawns the event loop.
//! The UI on top only sees the channel pair in [`UiSession`]: it sends
//! [`UiEvent`]s and applies the [`UiCommand`]s it receives. The ratatui
//! terminal and the native desktop app (`aura-desktop`) both run on it.

use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::model_selection::ModelSelection;
use crate::harness::{
    agent_tools, background_process_manager, build_hub, build_provider, initial_permission_mode,
    mode_policy, policy_tool_names, post_edit_check_from_env, project_instructions, Harness,
    HubSetup,
};
use crate::{api_server, event_loop, record_loader, session_helpers};

use anyhow::Context;
use aura_agent::AgentLoop;
use aura_agent_kernel::RecordObserver;
use aura_terminal::{UiCommand, UiEvent};
use aura_tools::BackgroundProcessHook;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Settings for the agent session behind a UI.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    /// Data directory for agents and records (default: `AURA_DATA_DIR` or
    /// `./aura_data`)
    pub dir: Option<PathBuf>,
    /// Model provider: `anthropic` (local Ollama models + aura-router) or
    /// `mock`
    pub provider: String,
    /// Permit FullAccess sessions to bypass command allowlists
    pub allow_unrestricted_full_access: bool,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            dir: None,
            provider: "anthropic".to_string(),
            allow_unrestricted_full_access: false,
        }
    }
}

/// A running agent session, as seen by the UI driving it.
pub struct UiSession {
    /// Where the UI sends user actions
    pub events: mpsc::Sender<UiEvent>,
    /// State updates for the UI, in order
    pub commands: mpsc::Receiver<UiCommand>,
    /// The event loop serving `events`
    pub task: SessionTask,
}

/// The spawned agent event loop of a [`UiSession`], and the background
/// commands its agents started.
pub struct SessionTask {
    handle: JoinHandle<anyhow::Result<()>>,
    background: Arc<dyn BackgroundProcessHook>,
}

impl SessionTask {
    /// Call once the UI has quit. Stops background commands, then
    /// surfaces the error when the event loop
    /// already stopped on its own (so the process exits non-zero), and
    /// aborts it when it is still running.
    ///
    /// # Errors
    ///
    /// The event loop's error, or a join error when it panicked.
    pub async fn finish(self) -> anyhow::Result<()> {
        event_loop::processes::stop_background_commands(self.background.as_ref());
        if !self.handle.is_finished() {
            self.handle.abort();
            return Ok(());
        }
        match self.handle.await {
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
    args: &SessionOptions,
    requested_mode: Option<aura_core_modes::AgentMode>,
) -> anyhow::Result<UiSession> {
    let (ui_tx, mut ui_rx) = mpsc::channel::<UiEvent>(100);
    let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>(200);

    let data_dir = args
        .dir
        .clone()
        .or_else(|| std::env::var("AURA_DATA_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("./aura_data"));

    // Tools run in the directory the UI was launched from, so typed
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

    let (process_manager, process_rx) = background_process_manager();

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
    let record_panel = event_loop::RecordPanelAgent::default();
    let record_router = Arc::new(aura_agent_trace::RecordRouter::new());
    let record_observer: Arc<dyn RecordObserver> = Arc::new(event_loop::FanoutObserver(vec![
        Arc::new(event_loop::UiRecordObserver::new(
            cmd_tx.clone(),
            record_panel.clone(),
        )),
        Arc::clone(&record_router) as Arc<dyn RecordObserver>,
    ]));
    let Harness { hub, background } = build_hub(HubSetup {
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
        record_observer: Some(record_observer),
        record_panel: record_panel.clone(),
        commands: cmd_tx.clone(),
        long_run: crate::long_run::LongRunSettings::from_env(),
        process_manager,
        background_agents: true,
        mode,
        instructions: project_instructions(),
        approvals: Some(Arc::clone(&approvals)),
        web_tools,
        post_edit_check: post_edit_check_from_env(),
        trace_snapshots: true,
    });
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
            memory_manager: None,
            models,
            auth,
            approvals,
            record_router,
        };
        event_loop::run_event_loop(ctx).await
    });

    Ok(UiSession {
        events: ui_tx,
        commands: cmd_rx,
        task: SessionTask {
            handle: processor_handle,
            background,
        },
    })
}
