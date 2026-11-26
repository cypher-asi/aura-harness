//! Agent session behind an interactive UI.
//!
//! [`start`] opens the agent registry, record store, model provider and
//! kernel factory for the current workspace and spawns the event loop.
//! The UI on top only sees the channel pair in [`UiSession`]: it sends
//! [`UiEvent`]s and applies the [`UiCommand`]s it receives. The ratatui
//! terminal and the native desktop app (`aura-desktop`) both run on it.

use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::harness::{
    agent_tools, background_process_manager, build_hub, build_provider, initial_approval_level,
    initial_permission_mode, mode_policy, policy_tool_names, post_edit_check_from_env,
    project_instructions, Harness, HubSetup,
};
use crate::model_selection::ModelSelection;
use crate::{api_server, event_loop, record_loader, session_helpers};

use anyhow::Context;
use aura_agent::AgentLoop;
use aura_agent_kernel::RecordObserver;
use aura_exec_tools::BackgroundProcessHook;
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
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            dir: None,
            provider: "anthropic".to_string(),
            allow_unrestricted_full_access: false,
            approval: None,
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
    let (ui_tx, mut ui_rx) = mpsc::channel::<UiEvent>(100);
    let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>(UI_COMMANDS);

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

    // The registry lives in the record store's database, so open that
    // first: RocksDB allows one handle per process on it.
    let store_path = session_helpers::resolve_store_path(&data_dir);
    let store = session_helpers::open_store(&store_path)
        .with_context(|| format!("opening {}", store_path.display()))?;
    let mut registry = AgentRegistry::open(
        aura_store_db::AgentStore::for_store(&store),
        &data_dir,
        &workspace_root,
    )
    .context("loading agent registry")?;
    let definitions =
        AgentDefinitions::load_or_seed(&data_dir).context("loading permanent agents")?;

    // A spawn the app closed or was killed on before its turn recorded
    // anything can't resume: there is nothing to resume from. Archive it
    // before the active agent is read, which skips archived agents, so the
    // app never opens on an empty chat it just archived.
    record_loader::archive_unstarted_spawns(&mut registry, store.as_ref());
    let active_agent = registry.active().clone();
    let identity = active_agent.identity();
    let active_folder = registry.project_folder(identity.agent_id);

    crate::chat_notes::install(crate::chat_notes::ChatNotes::new(&data_dir));
    record_loader::load_existing_records(store.as_ref(), identity.agent_id, &cmd_tx);
    match record_loader::conversation_history(store.as_ref(), identity.agent_id) {
        Ok(history) => {
            let history = crate::chat_notes::with_notes(identity.agent_id, history);
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
    let mut config = session_helpers::default_agent_config(aura_model::ENV_FALLBACK_MODEL);
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

    let permissions = event_loop::Permissions::new(
        initial_permission_mode(requested_mode),
        initial_approval_level(args.approval),
    );
    info!(permissions = %permissions.name(), "Terminal permissions");
    let tool_names = policy_tool_names(&tools);
    let approvals = Arc::new(event_loop::TerminalApprovals::new(
        cmd_tx.clone(),
        &data_dir,
    ));
    let record_panel = event_loop::RecordPanelAgent::default();
    let record_router = Arc::new(aura_agent_trace::RecordRouter::new());
    let (flow, flow_inbox) = event_loop::FlowFeed::new();
    let record_observer: Arc<dyn RecordObserver> = Arc::new(event_loop::FanoutObserver(vec![
        Arc::new(aura_agent_kernel::QueuedObserver::new(
            "aura-record-ui",
            Arc::new(event_loop::UiRecordObserver::new(
                cmd_tx.clone(),
                record_panel.clone(),
            )),
        )),
        Arc::clone(&record_router) as Arc<dyn RecordObserver>,
        Arc::clone(&flow) as Arc<dyn RecordObserver>,
    ]));
    let cloud_store = Arc::clone(&store) as Arc<dyn aura_store_db::Store>;
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
            gate: event_loop::GateBase::of(&models),
        },
        gate: aura_config::gate(),
        policy: mode_policy(permissions, &tool_names),
        record_observer: Some(record_observer),
        record_panel: record_panel.clone(),
        record_router: Some(record_router),
        trace_home: aura_config::AuraHome::resolve().ok().map(|home| home.path),
        commands: cmd_tx.clone(),
        long_run: crate::long_run::LongRunSettings::from_env(),
        process_manager,
        background_agents: true,
        permissions,
        instructions: project_instructions(),
        approvals: Some(Arc::clone(&approvals)),
        web_tools,
        post_edit_check: post_edit_check_from_env(),
        trace_snapshots: true,
    });
    hub.processes
        .keep_history(data_dir.join(PROCESS_HISTORY_FILE));
    flow_inbox.serve(&hub, cmd_tx.clone());
    for list in hub.agent_lists() {
        let _ = cmd_tx.try_send(list);
    }
    match event_loop::channels::channel_summaries(&hub).await {
        Ok(channels) if channels.is_empty() => {}
        Ok(channels) => {
            let _ = cmd_tx.try_send(UiCommand::SetChannels(channels));
        }
        Err(error) => tracing::warn!(error = %format!("{error:#}"), "Could not list the channels"),
    }
    let _ = cmd_tx.try_send(UiCommand::SetActiveAgent(identity.agent_id.to_hex()));
    let _ = cmd_tx.try_send(UiCommand::SetAgentDefinitions(hub.definition_summaries()));
    let session = hub.factory.open(&active_agent, Vec::new(), active_folder)?;
    record_panel.set(session.agent_id());

    let cmd_tx_clone = cmd_tx.clone();
    let quit = CancellationToken::new();
    let loop_quit = quit.clone();

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
            flow: Some(flow),
            quit: loop_quit,
            cloud_store: Some(cloud_store),
        };
        event_loop::run_event_loop(ctx).await
    });

    Ok(UiSession {
        events: ui_tx,
        commands: cmd_rx,
        task: SessionTask {
            handle: processor_handle,
            background,
            quit,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::AgentId;
    use std::future::Future;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// Started no background commands.
    struct NoBackground;

    impl BackgroundProcessHook for NoBackground {
        fn log_dir(&self) -> PathBuf {
            std::env::temp_dir()
        }

        fn adopt(
            &self,
            _agent: AgentId,
            _process: aura_exec_tools::BackgroundProcess,
        ) -> Result<(), String> {
            Err("not in this test".into())
        }
    }

    /// A session task whose event loop is `event_loop`, given the quit
    /// token.
    fn session_task<F, Fut>(event_loop: F) -> SessionTask
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let quit = CancellationToken::new();
        SessionTask {
            handle: tokio::spawn(event_loop(quit.clone())),
            background: Arc::new(NoBackground),
            quit,
        }
    }

    /// The bug behind "cancelled a turn, closed the app, and lost the
    /// data": `finish` aborted the loop while it was still running, so
    /// turns cancelled at quit never got to save their sessions.
    #[tokio::test(start_paused = true)]
    async fn finish_lets_the_event_loop_shut_down_instead_of_aborting_it() {
        let saved = Arc::new(AtomicBool::new(false));
        let loop_saved = Arc::clone(&saved);
        let task = session_task(move |quit| async move {
            quit.cancelled().await;
            // Its cancelled turns end and save their sessions.
            tokio::time::sleep(Duration::from_secs(2)).await;
            loop_saved.store(true, Ordering::SeqCst);
            Ok(())
        });

        task.finish().await.unwrap();

        assert!(
            saved.load(Ordering::SeqCst),
            "the loop was told to quit and allowed to finish"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn finish_aborts_an_event_loop_that_does_not_shut_down_in_time() {
        let (alive, aborted) = tokio::sync::oneshot::channel::<()>();
        let task = session_task(move |_quit| async move {
            // Dropped, closing the channel, when the loop is aborted.
            let _alive = alive;
            std::future::pending::<anyhow::Result<()>>().await
        });
        let started = tokio::time::Instant::now();

        task.finish().await.unwrap();

        assert!(started.elapsed() >= event_loop::FINISH_GRACE);
        let ended = tokio::time::timeout(Duration::from_secs(1), aborted).await;
        assert!(matches!(ended, Ok(Err(_))), "the stuck loop was aborted");
    }

    #[tokio::test(start_paused = true)]
    async fn finish_reports_an_event_loop_that_failed() {
        let task = session_task(|_quit| async { Err(anyhow::anyhow!("store closed")) });

        let error = task.finish().await.unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "agent event loop failed: store closed"
        );
    }
}
