//! Agent session behind an interactive UI.
//!
//! [`start`] opens the agent registry, record store, model provider and
//! kernel factory for the current workspace and spawns the event loop.
//! The UI on top only sees the channel pair in [`UiSession`]: it sends
//! [`UiEvent`]s and applies the [`UiCommand`]s it receives. The ratatui
//! terminal and the native desktop app (`aura-desktop`) both run on it.

use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::{AgentRegistry, RegisteredAgent};
use crate::harness::{
    agent_tools, background_process_manager, build_hub, build_provider, initial_approval_level,
    initial_permission_mode, mode_policy, policy_tool_names, post_edit_check_from_env,
    project_instructions, Harness, HubSetup,
};
use crate::long_run::WebTools;
use crate::model_selection::ModelSelection;
use crate::{api_server, event_loop, record_loader, session_helpers};

use anyhow::Context;
use aura_agent::{AgentLoop, AgentLoopConfig, ProcessManager};
use aura_agent_kernel::RecordObserver;
use aura_auth::AuthManager;
use aura_core_types::Transaction;
use aura_exec_tools::{BackgroundProcessHook, ToolConfig};
use aura_model::ModelProvider;
use aura_ui::{UiCommand, UiEvent};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Finished Processes tab rows from earlier sessions, in the data dir.
const PROCESS_HISTORY_FILE: &str = "process_history.json";

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
    /// Approval level agents start at (`--approval`); `None` reads
    /// `AURA_TUI_APPROVAL`, else `yolo`
    pub approval: Option<aura_ui::ApprovalLevel>,
    /// The chat the UI was in when it last closed, opened first; `None`
    /// opens on the registry's active agent
    pub open_on: Option<OpenOn>,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            dir: None,
            provider: "anthropic".to_string(),
            allow_unrestricted_full_access: false,
            approval: None,
            open_on: None,
        }
    }
}

pub use opening::{data_dir, OpenOn};

/// A running agent session, as seen by the UI driving it.
pub struct UiSession {
    /// Where the UI sends user actions
    pub events: mpsc::Sender<UiEvent>,
    /// State updates for the UI, in order
    pub commands: mpsc::Receiver<UiCommand>,
    /// The event loop serving `events`
    pub task: SessionTask,
    /// The data directory the session's agents and records are in, where
    /// a UI keeps what it saves between runs beside them
    pub data_dir: PathBuf,
}

/// The spawned agent event loop of a [`UiSession`], and the background
/// commands its agents started.
pub struct SessionTask {
    handle: JoinHandle<anyhow::Result<()>>,
    background: Arc<dyn BackgroundProcessHook>,
    /// Tells the event loop the UI is gone, so it shuts down
    quit: CancellationToken,
}

impl SessionTask {
    /// Call once the UI has quit. Stops background commands, then tells
    /// the event loop to shut down and waits for it: the loop cancels the
    /// agents' running turns and gives them time to end and save their
    /// sessions, which an aborted loop would lose. A loop still running
    /// after a few seconds more than that is aborted.
    ///
    /// # Errors
    ///
    /// The event loop's error, or a join error when it panicked.
    pub async fn finish(mut self) -> anyhow::Result<()> {
        event_loop::processes::stop_background_commands(self.background.as_ref());
        self.quit.cancel();
        let grace = event_loop::FINISH_GRACE;
        match tokio::time::timeout(grace, &mut self.handle).await {
            Ok(joined) => loop_result(joined),
            Err(_) => {
                warn!(
                    grace_ms = u64::try_from(grace.as_millis()).unwrap_or(u64::MAX),
                    "Agent event loop did not shut down in time; aborting it"
                );
                self.handle.abort();
                Ok(())
            }
        }
    }
}

/// What the event loop's task ending with `joined` means for the process:
/// a loop that failed or panicked makes it exit non-zero, and one that was
/// aborted is not a failure.
fn loop_result(joined: Result<anyhow::Result<()>, JoinError>) -> anyhow::Result<()> {
    match joined {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.context("agent event loop failed")),
        Err(join_err) if join_err.is_cancelled() => Ok(()),
        Err(join_err) => Err(anyhow::anyhow!(
            "agent event loop task did not join cleanly: {join_err}"
        )),
    }
}

/// Commands the UI may fall behind by before agents wait for it. Every
/// agent's turn sends through the one channel, a command per 33 ms of
/// streaming, so a slow frame (a long chat laid out the first time) must
/// not hold them all up.
const UI_COMMANDS: usize = 4096;

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
    let (ui_tx, ui_rx) = mpsc::channel::<UiEvent>(100);
    let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>(UI_COMMANDS);
    let (workspace, opening) = open_workspace(args, &cmd_tx).await?;
    api_server::start_api_server(cmd_tx.clone(), workspace.root.clone()).await;
    let (side, run, inbox) = agent_side(args, requested_mode, &cmd_tx, &workspace)?;
    let history = workspace.data_dir.join(PROCESS_HISTORY_FILE);
    let data_dir = workspace.data_dir.clone();
    let harness = build_hub(hub_setup(workspace, side, &run, &cmd_tx));
    let task = launch(harness, run, opening, inbox, ui_rx, cmd_tx, history).await?;
    Ok(UiSession {
        events: ui_tx,
        commands: cmd_rx,
        task,
        data_dir,
    })
}

/// The data directory, the record store in it and its agents.
struct Workspace {
    data_dir: PathBuf,
    /// Where tools run: the directory the UI was launched from
    root: PathBuf,
    store: Arc<aura_store_db::RocksStore>,
    registry: AgentRegistry,
    definitions: AgentDefinitions,
}

/// The agent the session opens on, with its project's folder and its
/// chat, `None` when its record could not be read.
struct Opening {
    agent: RegisteredAgent,
    folder: Option<PathBuf>,
    chat: Option<Vec<aura_ui::events::MessageData>>,
}

/// The workspace, and the agent the session opens on; the UI is sent
/// that agent's record now, and its chat by [`launch`].
async fn open_workspace(
    args: &SessionOptions,
    cmd_tx: &mpsc::Sender<UiCommand>,
) -> anyhow::Result<(Workspace, Opening)> {
    let data_dir = data_dir(args);
    // Tools run in the directory the UI was launched from, so typed
    // commands and model tool calls see the user's project rather than an
    // empty per-agent scratch dir under `data_dir`.
    let root = std::env::current_dir().context("resolving working directory")?;
    tokio::fs::create_dir_all(&data_dir).await?;
    // The registry lives in the record store's database, so open that
    // first: RocksDB allows one handle per process on it.
    let store_path = session_helpers::resolve_store_path(&data_dir);
    let store = crate::local_store::open(&data_dir, &store_path)
        .await
        .with_context(|| format!("opening {}", store_path.display()))?;
    // After the store: it made the key the integrations seal with.
    crate::integrations::open(&data_dir);
    let agents = aura_store_db::AgentStore::for_store(&store);
    let mut registry =
        AgentRegistry::open(agents, &data_dir, &root).context("loading agent registry")?;
    let definitions =
        AgentDefinitions::load_or_seed(&data_dir).context("loading permanent agents")?;
    // A spawn the app closed or was killed on before its turn recorded
    // anything can't resume: there is nothing to resume from. Archive it
    // before the active agent is read, which skips archived agents, so the
    // app never opens on an empty chat it just archived.
    record_loader::archive_unstarted_spawns(&mut registry, store.as_ref());
    let active_agent = opening::agent_to_open(&mut registry, &store, args.open_on.as_ref());
    let agent_id = active_agent.identity().agent_id;
    let active_folder = registry.project_folder(agent_id);
    crate::chat_notes::install(crate::chat_notes::ChatNotes::new(&data_dir));
    record_loader::load_existing_records(store.as_ref(), agent_id, cmd_tx);
    let chat = record_loader::conversation_history(store.as_ref(), agent_id)
        .inspect_err(|e| warn!(error = %e, "Failed to rebuild conversation from record"))
        .ok()
        .map(|history| crate::chat_notes::with_notes(agent_id, history));
    let workspace = Workspace {
        data_dir,
        root,
        store,
        registry,
        definitions,
    };
    let opening = Opening {
        agent: active_agent,
        folder: active_folder,
        chat,
    };
    Ok((workspace, opening))
}

/// What [`hub_setup`] hands the hub from [`agent_side`].
struct HubSide {
    tools: (ToolConfig, Vec<aura_model::ToolDefinition>, WebTools),
    config: AgentLoopConfig,
    provider: Arc<dyn ModelProvider + Send + Sync>,
    permissions: event_loop::Permissions,
    process_manager: Arc<ProcessManager>,
}

/// What [`launch`] runs the event loop on, besides the hub.
struct LoopSide {
    agent_loop: AgentLoop,
    process_completions: mpsc::Receiver<Transaction>,
    models: ModelSelection,
    auth: Arc<AuthManager>,
    approvals: Arc<event_loop::TerminalApprovals>,
    flow: Arc<event_loop::FlowFeed>,
    record_panel: event_loop::RecordPanelAgent,
    cloud_store: Arc<dyn aura_store_db::Store>,
}

/// The tools, model, provider and permissions of the session's agents.
fn agent_side(
    args: &SessionOptions,
    requested_mode: Option<aura_core_modes::AgentMode>,
    cmd_tx: &mpsc::Sender<UiCommand>,
    workspace: &Workspace,
) -> anyhow::Result<(HubSide, LoopSide, event_loop::FlowInbox)> {
    let tools = agent_tools(args.allow_unrestricted_full_access);
    let (config, auth, models) = select_model()?;
    let agent_loop = AgentLoop::new(config.clone());
    let (process_manager, process_completions) = background_process_manager();
    // `mock` for explicit test / offline mode; otherwise the routed
    // provider sends `local/*` models to the local server (Ollama),
    // `claude/*` and `chatgpt/*` to the linked plan, and everything else
    // to aura-router.
    let selection = build_provider(&args.provider, &auth)?;
    if selection.name == "mock" {
        let _ = cmd_tx.try_send(UiCommand::SetStatus("Mock Mode".to_string()));
    }
    let permissions = event_loop::Permissions::new(
        initial_permission_mode(requested_mode),
        initial_approval_level(args.approval),
    );
    info!(permissions = %permissions.name(), "Terminal permissions");
    let approvals = event_loop::TerminalApprovals::new(cmd_tx.clone(), &workspace.data_dir);
    let (flow, flow_inbox) = event_loop::FlowFeed::new();
    let hub = HubSide {
        tools,
        config,
        provider: selection.provider,
        permissions,
        process_manager,
    };
    let run = LoopSide {
        agent_loop,
        process_completions,
        models,
        auth,
        approvals: Arc::new(approvals),
        flow,
        record_panel: event_loop::RecordPanelAgent::default(),
        cloud_store: Arc::clone(&workspace.store) as Arc<dyn aura_store_db::Store>,
    };
    Ok((hub, run, flow_inbox))
}

/// The agent config, sign-in state and model terminal agents start with.
fn select_model() -> anyhow::Result<(AgentLoopConfig, Arc<AuthManager>, ModelSelection)> {
    // The terminal harness is the one surface that resolves its model
    // locally (env, persisted `/model` choice, then the local default) —
    // higher-level surfaces (chat WS, dev-loop, task-run) plumb the
    // user-selected model through their own paths.
    let mut config = session_helpers::default_agent_config(aura_model::ENV_FALLBACK_MODEL);
    let auth = Arc::new(AuthManager::new().context("loading sign-in state")?);
    let summary = event_loop::auth::auth_summary(&auth, config.auth_token.as_deref());
    let models = ModelSelection::init(&mut config, summary);
    info!(model = %models.current(), "Terminal model selected");
    Ok((config, auth, models))
}

/// The hub's settings, observing records for the panel, traces and GRAPH.
fn hub_setup(
    workspace: Workspace,
    side: HubSide,
    run: &LoopSide,
    cmd_tx: &mpsc::Sender<UiCommand>,
) -> HubSetup {
    let (tool_config, tools, web_tools) = side.tools;
    let tool_names = policy_tool_names(&tools);
    let record_router = Arc::new(aura_agent_trace::RecordRouter::new());
    let ui = event_loop::UiRecordObserver::new(cmd_tx.clone(), run.record_panel.clone());
    let record_observer: Arc<dyn RecordObserver> = Arc::new(event_loop::FanoutObserver(vec![
        Arc::new(aura_agent_kernel::QueuedObserver::new(
            "aura-record-ui",
            Arc::new(ui),
        )),
        Arc::clone(&record_router) as Arc<dyn RecordObserver>,
        Arc::clone(&run.flow) as Arc<dyn RecordObserver>,
    ]));
    HubSetup {
        data_dir: workspace.data_dir,
        workspace_root: workspace.root,
        registry: workspace.registry,
        definitions: workspace.definitions,
        store: workspace.store,
        provider: side.provider,
        tool_config,
        base: event_loop::TurnBase {
            config: side.config,
            tools,
            limits: run.models.limits(),
            gate: event_loop::GateBase::of(&run.models),
        },
        gate: aura_config::gate(),
        policy: mode_policy(side.permissions, &tool_names),
        record_observer: Some(record_observer),
        record_panel: run.record_panel.clone(),
        record_router: Some(record_router),
        trace_home: aura_config::AuraHome::resolve().ok().map(|home| home.path),
        commands: cmd_tx.clone(),
        long_run: crate::long_run::LongRunSettings::from_env(),
        process_manager: side.process_manager,
        background_agents: true,
        permissions: side.permissions,
        instructions: project_instructions(),
        approvals: Some(Arc::clone(&run.approvals)),
        web_tools,
        post_edit_check: post_edit_check_from_env(),
        trace_snapshots: true,
    }
}

/// Send the UI the hub's agents and channels, open the active agent and
/// spawn the event loop serving `events`.
async fn launch(
    harness: Harness,
    run: LoopSide,
    opening: Opening,
    flow_inbox: event_loop::FlowInbox,
    mut events: mpsc::Receiver<UiEvent>,
    commands: mpsc::Sender<UiCommand>,
    process_history: PathBuf,
) -> anyhow::Result<SessionTask> {
    let hub = harness.hub;
    hub.processes.keep_history(process_history);
    flow_inbox.serve(&hub, commands.clone());
    let Opening {
        agent: active_agent,
        folder: active_folder,
        chat,
    } = opening;
    show_opening(&hub, &commands, active_agent.identity().agent_id, chat).await;
    let session = hub.factory.open(&active_agent, Vec::new(), active_folder)?;
    run.record_panel.set(session.agent_id());
    let quit = CancellationToken::new();
    let loop_quit = quit.clone();
    let handle = tokio::spawn(async move {
        let mut agent_loop = run.agent_loop;
        let ctx = event_loop::EventLoopContext {
            events: &mut events,
            process_completions: run.process_completions,
            commands,
            agent_loop: &mut agent_loop,
            session,
            hub,
            memory_manager: None,
            models: run.models,
            auth: run.auth,
            approvals: run.approvals,
            flow: Some(run.flow),
            quit: loop_quit,
            cloud_store: Some(run.cloud_store),
        };
        event_loop::run_event_loop(ctx).await
    });
    Ok(SessionTask {
        handle,
        background: harness.background,
        quit,
    })
}

/// Send the UI the lists, the channels and `agent_id`, the agent the
/// session opens on, with its `chat`.
async fn show_opening(
    hub: &event_loop::AgentHub,
    commands: &mpsc::Sender<UiCommand>,
    agent_id: aura_core_types::AgentId,
    chat: Option<Vec<aura_ui::events::MessageData>>,
) {
    for list in hub.agent_lists() {
        let _ = commands.try_send(list);
    }
    match event_loop::channels::channel_summaries(hub).await {
        Ok(channels) if channels.is_empty() => {}
        Ok(channels) => {
            let _ = commands.try_send(UiCommand::SetChannels(channels));
        }
        Err(error) => tracing::warn!(error = %format!("{error:#}"), "Could not list the channels"),
    }
    let _ = commands.try_send(UiCommand::SetActiveAgent(agent_id.to_hex()));
    // Addressed, and only after the lists and the agent: a desktop window
    // gives it to the tab holding the agent, which the lists decide.
    if let Some(chat) = chat {
        let _ = commands.try_send(UiCommand::ForAgent {
            agent_id: agent_id.to_hex(),
            command: Box::new(UiCommand::LoadConversation(chat)),
        });
    }
    let _ = commands.try_send(UiCommand::SetAgentDefinitions(hub.definition_summaries()));
}

mod opening;
#[cfg(test)]
mod tests;
