//! Event processing loop for the terminal UI mode.
//!
//! Agents work in parallel: each turn runs on its own task (see
//! [`turns`]), so the loop keeps handling the UI while agents work, and the
//! user can switch to another agent and give it work too. Each agent does
//! one thing at a time. A message for an agent in the middle of a turn
//! goes into that turn (see [`steering`]); other work waits for it. So
//! do notices from work running in the background (see [`notices`]):
//! commands, `task` spawns and agents the agent started, all listed in
//! the Processes tab (see [`processes`]).

pub(crate) mod agent_events;
pub(crate) mod agents;
pub(crate) mod approvals;
mod attachments;
pub mod auth;
pub(crate) mod handlers;
pub(crate) mod host;
pub(crate) mod hud;
pub(crate) mod modes;
pub(crate) use modes::Permissions;
pub(crate) mod notices;
pub(crate) mod os_processes;
pub(crate) mod plan_mode;
pub(crate) mod processes;
pub(crate) mod record_ui;
mod rewind;
pub(crate) mod run_command;
pub(crate) mod steering;
pub(crate) mod subagents;
mod titles;
pub(crate) mod trace;
pub(crate) mod turns;

pub(crate) use agent_events::forward_agent_events;
pub use agents::{AgentSession, KernelBuilder, KernelFactory};
pub use approvals::TerminalApprovals;
pub use host::{TerminalAgents, TerminalChildKernels};
pub use processes::{BackgroundCommands, ProcessTable};
pub use record_ui::{FanoutObserver, RecordPanelAgent, UiRecordObserver};
pub use subagents::{AgentHub, AgentHubParts, TurnBase};
pub use trace::Traces;

use crate::agent_registry::RegisteredAgent;
use crate::model_selection::ModelSelection;
use agent_events::AgentUi;
use approvals::Answer;
use aura_agent::AgentLoop;
use aura_agent_kernel::Kernel;
use aura_agent_trace::RunRecorder;
use aura_auth::{AuthManager, PendingLink};
use aura_core_types::{AgentId, Transaction};
use aura_model::model_catalog::local_model_name;
use aura_model::ToolDefinition;
use aura_ui_terminal::events::{ImageAttachment, ProcessStatus};
use aura_ui_terminal::{PermissionMode, UiCommand, UiEvent};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use steering::{SteerSource, Steering};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use turns::{Prompt, Task, Work, WorkDone};

/// How long quitting waits for cancelled turns to record how they ended.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Bundled dependencies for the event loop, reducing parameter count.
pub struct EventLoopContext<'a> {
    pub events: &'a mut mpsc::Receiver<UiEvent>,
    pub process_completions: mpsc::Receiver<Transaction>,
    pub commands: mpsc::Sender<UiCommand>,
    pub agent_loop: &'a mut AgentLoop,
    /// Session for the agent shown when the loop starts.
    pub session: AgentSession,
    /// Agents, permanent agents, the kernel factory and the Processes
    /// tab's table, shared with spawns.
    pub hub: Arc<AgentHub>,
    /// Optional memory manager for prompt injection and result ingestion.
    pub memory_manager: Option<Arc<aura_context_memory::MemoryManager>>,
    /// Model selection already applied to `agent_loop`.
    pub models: ModelSelection,
    /// Sign-in state shared with the model provider.
    pub auth: Arc<AuthManager>,
    /// Tool calls waiting for the user's answer.
    pub approvals: Arc<TerminalApprovals>,
}

/// Mutable state threaded through all event handlers.
pub(super) struct LoopState<'a> {
    pub(super) commands: &'a mpsc::Sender<UiCommand>,
    pub(super) events: &'a mut mpsc::Receiver<UiEvent>,
    pub(super) agent_loop: &'a mut AgentLoop,
    pub(super) memory_manager: Option<Arc<aura_context_memory::MemoryManager>>,
    pub(super) models: ModelSelection,
    /// The agent the UI is showing.
    pub(super) active: AgentId,
    /// Sessions of the opened agents with no work in flight. An agent at
    /// work has its session on the task doing the work.
    pub(super) idle: HashMap<AgentId, AgentSession>,
    /// Work waiting for a busy agent, oldest first
    queued: HashMap<AgentId, VecDeque<Work>>,
    /// Where agent work reports that it ended
    work_done: mpsc::Sender<WorkDone>,
    /// Work started by the loop that hasn't ended yet
    in_flight: usize,
    pub(super) hub: Arc<AgentHub>,
    pub(super) processes: Arc<ProcessTable>,
    pub(super) auth: Arc<AuthManager>,
    /// Plan sign-in waiting for a pasted code.
    pub(super) pending_link: Option<PendingLink>,
    pub(super) approvals: Arc<TerminalApprovals>,
}

impl LoopState<'_> {
    /// Kernel of `agent_id`, whether it is idle or at work.
    pub(super) fn kernel_for(&self, agent_id: AgentId) -> Option<Arc<Kernel>> {
        self.idle
            .get(&agent_id)
            .map(|s| Arc::clone(&s.kernel))
            .or_else(|| self.hub.running_kernel(agent_id))
    }

    /// Keep the sessions of finished spawns so switching to them resumes
    /// their conversation on the kernel that wrote it, then start any work
    /// that was waiting for them.
    async fn park_finished_spawns(&mut self) {
        for session in self.hub.take_finished() {
            let agent_id = session.agent_id();
            self.idle.insert(agent_id, session);
            start_next(self, agent_id).await;
        }
    }

    /// Publish the terminal's current config as the base for spawned agents.
    fn sync_turn_base(&mut self) {
        let config = self.agent_loop.config_mut().clone();
        let mut base = subagents::lock(&self.hub.base);
        base.config = config;
        base.limits = self.models.limits();
    }

    /// The agent named by hex id `agent_hex`, or the active agent when the
    /// UI didn't name a valid one.
    fn agent_or_active(&self, agent_hex: &str) -> AgentId {
        AgentId::from_hex(agent_hex).unwrap_or(self.active)
    }
}

/// Run the event processing loop.
///
/// Handles user messages from the UI and process completion events.
pub async fn run_event_loop(ctx: EventLoopContext<'_>) -> anyhow::Result<()> {
    let EventLoopContext {
        events,
        mut process_completions,
        commands,
        agent_loop,
        session,
        hub,
        memory_manager,
        models,
        auth,
        approvals,
    } = ctx;
    let processes = Arc::clone(&hub.processes);
    let publisher = processes::spawn_publisher(Arc::clone(&processes), commands.clone());
    let (link_tx, mut link_rx) = mpsc::channel::<auth::LinkOutcome>(4);
    let (done_tx, mut done_rx) = mpsc::channel::<WorkDone>(64);

    let active = session.agent_id();
    let mut state = LoopState {
        commands: &commands,
        events,
        agent_loop,
        memory_manager,
        models,
        active,
        idle: HashMap::from([(active, session)]),
        queued: HashMap::new(),
        work_done: done_tx,
        in_flight: 0,
        hub,
        processes,
        auth,
        pending_link: None,
        approvals,
    };
    let mut notice_rx = state.hub.take_notices();
    state.hub.name_agents();
    state.hub.record_panel.set(active);

    agents::send_sandbox_root(&state).await;
    hud::send_session_of(&state, active).await;
    modes::send_mode(&state, active).await;
    modes::warn_if_unconfined(&state, active).await;
    hud::send_instructions_of(&state, active).await;
    plan_mode::send_plan_of(&state, active).await;
    state.hub.traces.send_history(active).await;
    handlers::announce_models(&mut state).await;

    let hub = Arc::clone(&state.hub);
    loop {
        tokio::select! {
            Some(done) = done_rx.recv() => {
                handle_work_done(&mut state, done).await;
            }
            () = hub.spawn_finished.notified() => {
                state.park_finished_spawns().await;
                agents::send_agent_list(&state).await;
            }
            Some(completion_tx) = process_completions.recv() => {
                handle_completion(&mut state, completion_tx).await;
            }
            Some(notice) = notice_rx.recv() => {
                notices::handle_notice(&mut state, notice).await;
            }
            Some(outcome) = link_rx.recv() => {
                auth::handle_link_outcome(&mut state, outcome).await;
            }
            event = state.events.recv() => {
                let Some(event) = event else { break };
                if handle_ui_event(&mut state, event, &link_tx).await {
                    break;
                }
            }
        }
    }
    shut_down(&mut state, &mut done_rx).await;
    publisher.abort();
    state.processes.save();
    state.hub.traces.close_all();
    Ok(())
}

/// Give `text` from `source` to `agent_id`: into its running turn if it
/// has one that still takes input, else as a prompt. `shown` is whether
/// the chat already shows it.
async fn deliver(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    source: SteerSource,
    text: String,
    shown: bool,
) {
    deliver_with_images(state, agent_id, source, text, Vec::new(), shown).await;
}

/// [`deliver`] for a user message with pasted `images`. A running turn
/// only takes text, so a message with images waits for the next turn.
async fn deliver_with_images(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    source: SteerSource,
    text: String,
    images: Vec<ImageAttachment>,
    shown: bool,
) {
    if !images.is_empty() {
        let work = Work::Prompt {
            prompt: Prompt::typed(text, images),
            shown,
        };
        submit(state, agent_id, work).await;
        return;
    }
    if let Some(steering) = state.hub.steering(agent_id) {
        let body = steered_body(state, agent_id, source, &text).await;
        if steering.send(source, text.clone(), body).await {
            info!(%agent_id, ?source, "Sent message into the running turn");
            if source == SteerSource::User && !shown {
                AgentUi::new(state.commands.clone(), agent_id)
                    .send(UiCommand::UserSteered(text))
                    .await;
            }
            return;
        }
    }
    submit(state, agent_id, prompt_work(source, text, shown)).await;
}

/// What the model gets for `text` sent into a running turn: a user's
/// message has the files it mentions appended, read from the agent's
/// workspace.
async fn steered_body(
    state: &LoopState<'_>,
    agent_id: AgentId,
    source: SteerSource,
    text: &str,
) -> String {
    if source != SteerSource::User || attachments::mentioned_paths(text).is_empty() {
        return text.to_string();
    }
    let Some(kernel) = state.kernel_for(agent_id) else {
        return text.to_string();
    };
    attachments::expand_mentions_in(kernel.agent_workspace(), text.to_string())
        .await
        .text
}

/// A prompt from `source`. Harness notices keep their label so the model
/// can tell them from the user.
fn prompt_work(source: SteerSource, text: String, shown: bool) -> Work {
    match source {
        SteerSource::User => Work::Prompt {
            prompt: Prompt::typed(text, Vec::new()),
            shown,
        },
        SteerSource::Harness => Work::Prompt {
            prompt: Prompt::plain(format!("{}{text}", source.prefix())),
            shown: false,
        },
    }
}

/// Cancel every agent's work and give it a moment to record how it ended.
async fn shut_down(state: &mut LoopState<'_>, done_rx: &mut mpsc::Receiver<WorkDone>) {
    state.queued.clear();
    state.approvals.cancel_all();
    state.hub.cancel_all();
    let deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE;
    while state.in_flight > 0 {
        match tokio::time::timeout_at(deadline, done_rx.recv()).await {
            Ok(Some(_)) => state.in_flight -= 1,
            Ok(None) | Err(_) => {
                warn!(
                    in_flight = state.in_flight,
                    "Quitting before every agent stopped"
                );
                break;
            }
        }
    }
}

/// A background command ended: record it for the agent that started it,
/// and tell the agent.
async fn handle_completion(state: &mut LoopState<'_>, completion_tx: Transaction) {
    info!(
        hash = %completion_tx.hash,
        tx_type = ?completion_tx.tx_type,
        "Processing async process completion"
    );
    let notice = notices::completion_notice(&state.processes, &completion_tx);
    record_completion(state, completion_tx).await;
    if let Some(notice) = notice {
        notices::handle_notice(state, notice).await;
    }
}

/// Write a process completion to the record of the agent that started it.
async fn record_completion(state: &mut LoopState<'_>, completion_tx: Transaction) {
    let agent_id = completion_tx.agent_id;
    let Some(kernel) = state.kernel_for(agent_id) else {
        warn!(%agent_id, "Process completion for an agent with no open kernel");
        return;
    };
    // The kernels' record observer shows the entry.
    if notices::record_completion(&kernel, &completion_tx).await {
        if agent_id == state.active {
            let _ = state
                .commands
                .send(UiCommand::SetStatus("Process completed".to_string()))
                .await;
        } else {
            agents::send_agent_list(state).await;
        }
    }
}

/// Returns `true` if the loop should break (quit).
async fn handle_ui_event(
    state: &mut LoopState<'_>,
    event: UiEvent,
    link_outcomes: &mpsc::Sender<auth::LinkOutcome>,
) -> bool {
    match event {
        UiEvent::UserMessage {
            agent_id,
            text,
            shown,
            images,
        } => {
            let agent_id = state.agent_or_active(&agent_id);
            deliver_with_images(state, agent_id, SteerSource::User, text, images, shown).await;
        }
        UiEvent::RunCommand {
            agent_id,
            command,
            cols,
        } => {
            let agent_id = state.agent_or_active(&agent_id);
            submit(state, agent_id, Work::Command { command, cols }).await;
        }
        UiEvent::Approve(id) => answer_approval(state, &id, Answer::Once).await,
        UiEvent::ApproveAlways(id) => answer_approval(state, &id, Answer::Always).await,
        UiEvent::Deny(id) => answer_approval(state, &id, Answer::Deny).await,
        UiEvent::SetMode { agent_id, mode } => {
            let agent_id = state.agent_or_active(&agent_id);
            modes::set_mode(state, agent_id, mode).await;
        }
        UiEvent::SetApproval { agent_id, level } => {
            let agent_id = state.agent_or_active(&agent_id);
            modes::set_approval(state, agent_id, level).await;
        }
        UiEvent::CycleApproval(agent_id) => {
            let agent_id = state.agent_or_active(&agent_id);
            let next = state.hub.permissions_of(agent_id).approval.next();
            modes::set_approval(state, agent_id, next).await;
        }
        UiEvent::TogglePlan(agent_id) => {
            let agent_id = state.agent_or_active(&agent_id);
            let next = if state.hub.mode_of(agent_id) == PermissionMode::Plan {
                state.hub.exit_plan_mode(agent_id)
            } else {
                PermissionMode::Plan
            };
            modes::set_mode(state, agent_id, next).await;
        }
        UiEvent::Quit => {
            debug!("Quit received");
            return true;
        }
        UiEvent::Cancel(agent_id) => {
            let agent_id = state.agent_or_active(&agent_id);
            debug!(%agent_id, "Cancel received");
            // A call waiting for an answer holds the turn; deny it so the
            // turn sees the cancellation.
            state.approvals.cancel_agent(agent_id).await;
            if !state.hub.cancel(agent_id) {
                AgentUi::new(state.commands.clone(), agent_id)
                    .send(UiCommand::SetStatus("Cancelled".to_string()))
                    .await;
            }
        }
        UiEvent::ShowStatus => trace::status_report(state).await,
        UiEvent::OpenGraph(id) => trace::open_graph(state, id).await,
        UiEvent::ListRuns => trace::list(state).await,
        UiEvent::LabelStep { run_id, seq, value } => {
            trace::label(state, &run_id, seq, value).await;
        }
        UiEvent::ShowHelp | UiEvent::ShowHistory(_) => {}
        UiEvent::Clear => {
            if handlers::handle_new_session(state).await {
                let _ = state.commands.send(UiCommand::ClearConversation).await;
            }
        }
        UiEvent::NewSession => {
            handlers::handle_new_session(state).await;
        }
        UiEvent::SelectAgent(agent_hex) => agents::handle_select_agent(state, &agent_hex).await,
        UiEvent::FocusAgent(agent_hex) => agents::handle_focus_agent(state, &agent_hex).await,
        UiEvent::CreateAgent(name) => agents::handle_create_agent(state, name).await,
        UiEvent::CreateProject { name, folder } => {
            agents::handle_create_project(state, &name, &folder).await;
        }
        UiEvent::RenameProject(name) => agents::handle_rename_project(state, &name).await,
        UiEvent::AddAgentToProject(root) => {
            agents::handle_add_agent_to_project(state, &root).await;
        }
        UiEvent::ArchiveAgent(agent) => agents::handle_archive_agent(state, &agent).await,
        UiEvent::RestoreAgent(agent) => agents::handle_restore_agent(state, &agent).await,
        UiEvent::RemoveProject(agent) => agents::handle_remove_project(state, &agent).await,
        UiEvent::RefreshAgents => agents::handle_refresh_agents(state).await,
        UiEvent::InstantiateDefinition(name) => {
            agents::handle_instantiate_definition(state, &name).await;
        }
        UiEvent::AddDefinitionToProject(name) => {
            agents::handle_add_definition_to_project(state, &name).await;
        }
        UiEvent::EditDefinition(name) => agents::handle_edit_definition(state, name).await,
        UiEvent::DeleteDefinition(name) => agents::handle_delete_definition(state, &name).await,
        UiEvent::DefinitionEdited(path) => agents::handle_definition_edited(state, &path).await,
        UiEvent::LoginCredentials { email, password } => {
            auth::handle_login(state, &email, &password).await;
        }
        UiEvent::Logout => auth::handle_logout(state).await,
        UiEvent::Whoami => auth::handle_whoami(state).await,
        UiEvent::LinkSubscription { provider, paste } => {
            auth::handle_link(state, &provider, paste, link_outcomes).await;
        }
        UiEvent::LinkCode(code) => auth::handle_link_code(state, &code).await,
        UiEvent::Unlink(provider) => auth::handle_unlink(state, &provider).await,
        UiEvent::SelectModel(id) => handlers::handle_select_model(state, &id).await,
        UiEvent::Resume { agent_id, index } => {
            let agent_id = state.agent_or_active(&agent_id);
            handlers::handle_resume(state, agent_id, index).await;
        }
        UiEvent::Compact { agent_id, focus } => {
            let agent_id = state.agent_or_active(&agent_id);
            submit(state, agent_id, Work::Compact { focus }).await;
        }
        UiEvent::InitProject { agent_id } => {
            let agent_id = state.agent_or_active(&agent_id);
            handlers::handle_init_project(state, agent_id).await;
        }
        UiEvent::Rewind {
            agent_id,
            turn,
            scope,
        } => {
            let agent_id = state.agent_or_active(&agent_id);
            rewind::handle_rewind(state, agent_id, turn, scope).await;
        }
        UiEvent::Diff { agent_id, session } => {
            let agent_id = state.agent_or_active(&agent_id);
            rewind::handle_diff(state, agent_id, session).await;
        }
        UiEvent::KillProcess(id) => handlers::handle_kill_process(state, &id).await,
        UiEvent::BuildPlan { agent_id, path } => {
            let agent_id = state.agent_or_active(&agent_id);
            submit(state, agent_id, Work::Build { path }).await;
        }
    }
    false
}

/// Hand the user's answer to the tool call waiting as `request_id`.
async fn answer_approval(state: &LoopState<'_>, request_id: &str, answer: Answer) {
    debug!(request = %request_id, ?answer, "Approval answered");
    if let Err(e) = state.approvals.answer(request_id, answer).await {
        let _ = state
            .commands
            .send(UiCommand::ShowWarning(e.to_string()))
            .await;
    }
}

/// Give `work` to `agent_id`: now if it is idle, else once its current
/// work ends.
async fn submit(state: &mut LoopState<'_>, agent_id: AgentId, work: Work) {
    if state.hub.is_running(agent_id) {
        info!(%agent_id, "Agent is busy; queueing work");
        state.queued.entry(agent_id).or_default().push_back(work);
        hud::send_queued(state, agent_id).await;
        return;
    }
    start_work(state, agent_id, work).await;
}

/// Start the oldest work waiting for `agent_id`, if any.
async fn start_next(state: &mut LoopState<'_>, agent_id: AgentId) {
    let Some(queue) = state.queued.get_mut(&agent_id) else {
        return;
    };
    let next = queue.pop_front();
    if queue.is_empty() {
        state.queued.remove(&agent_id);
    }
    hud::send_queued(state, agent_id).await;
    if let Some(work) = next {
        start_work(state, agent_id, work).await;
    }
}

/// Run `work` on `agent_id` in the background.
async fn start_work(state: &mut LoopState<'_>, agent_id: AgentId, work: Work) {
    let ui = AgentUi::new(state.commands.clone(), agent_id);
    let mut session = match state.idle.remove(&agent_id) {
        Some(session) => session,
        None => match agents::open_session(state, agent_id) {
            Ok(session) => session,
            Err(problem) => {
                ui.send(UiCommand::ShowError(problem)).await;
                ui.send(UiCommand::Complete).await;
                return;
            }
        },
    };

    // A session opened from the record has a kernel built in the startup
    // mode; the agent's own mode decides its calls.
    modes::apply(
        &session.kernel,
        state.hub.permissions_of(agent_id),
        &state.hub.tool_names(),
    );
    let work = match work {
        Work::Build { path } => match plan_mode::start_build(state, &mut session, path).await {
            Some(text) => Work::Prompt {
                prompt: Prompt::plain(text),
                shown: false,
            },
            None => {
                state.idle.insert(agent_id, session);
                return;
            }
        },
        other => other,
    };
    let ui = ui.with_shown(state.hub.record_panel.clone());
    let cancel = CancellationToken::new();
    let mut steering = None;
    let mut row = None;
    let task = match work {
        Work::Prompt { prompt, shown } => {
            info!(%agent_id, text = %prompt.text, "Processing user message");
            // A local model with no server used to sit there until the
            // 10-minute request timeout; the probe answers in well under a
            // second.
            if local_model_unavailable(state, &ui).await {
                state.idle.insert(agent_id, session);
                return;
            }
            ui.send(UiCommand::TurnStarted {
                prompt: (!shown).then(|| prompt.text.clone()),
            })
            .await;
            let (agent_loop, tools, recorder) = prepare_turn(state, &session).await;
            if let Some(rec) = &recorder {
                rec.begin_turn(&prompt.text);
            }
            row = Some(handlers::turn_summary(
                agent_id,
                &prompt.text,
                &agent_loop.config().model,
            ));
            let turn_steering = Steering::new(&cancel);
            steering = Some(turn_steering.clone());
            let title_model = (prompt.mentions
                && subagents::lock(&state.hub.registry)
                    .find(agent_id)
                    .is_some_and(RegisteredAgent::needs_title))
            .then(|| titles::title_model(&agent_loop.config().model));
            Task::Turn {
                prompt,
                agent_loop: Box::new(agent_loop),
                tools,
                steering: turn_steering,
                limits: state.hub.long_run,
                recorder,
                title_model,
            }
        }
        Work::Command { command, cols } => Task::Command { command, cols },
        Work::Compact { focus } => {
            state.sync_turn_base();
            let (config, _) = state.hub.turn(
                session.definition.as_deref(),
                &session.kernel.agent_workspace(),
            );
            row = Some(handlers::turn_summary(
                agent_id,
                "Summarizing older history",
                &config.model,
            ));
            Task::Compact {
                agent_loop: Box::new(AgentLoop::new(config)),
                focus,
            }
        }
        Work::Build { .. } => return,
    };

    if let Some(row) = row {
        state.processes.start(row, Some(cancel.clone()));
    }
    state.hub.start_running(
        agent_id,
        Arc::clone(&session.kernel),
        cancel.clone(),
        steering,
    );
    state.in_flight += 1;
    turns::spawn(session, task, ui, cancel, state.work_done.clone());
    agents::send_agent_list(state).await;
}

/// The agent loop and tools for a turn of `session`, and the recorder of
/// its conversation's run when tracing is on.
async fn prepare_turn(
    state: &mut LoopState<'_>,
    session: &AgentSession,
) -> (AgentLoop, Vec<ToolDefinition>, Option<Arc<RunRecorder>>) {
    for problem in subagents::lock(&state.hub.definitions).reload() {
        warn!(%problem, "Skipping agent definition");
    }
    state.hub.send_definitions().await;
    state.sync_turn_base();
    let workspace = session.kernel.agent_workspace();
    let agent_id = session.agent_id();
    let (mut config, tools) =
        state
            .hub
            .agent_turn(agent_id, session.definition.as_deref(), &workspace);
    state.hub.send_instructions(agent_id, &workspace).await;
    state
        .hub
        .register_root(agent_id, &session.identity.name, workspace, &config);
    config.initial_todos.clone_from(&session.todos);
    if let Some(ref mm) = state.memory_manager {
        mm.prepare_context(agent_id, &mut config.system_prompt)
            .await;
        config.observers.clear();
        config
            .observers
            .push(aura_engine::memory_observer::MemoryTurnObserver::new(
                Arc::clone(mm),
                session.agent_id(),
                None,
                Vec::new(),
                None,
            ));
    }
    let recorder = trace::recorder(state, session, &config.model);
    if let Some(rec) = &recorder {
        config.trace_blobs = rec.blob_store();
    }
    (AgentLoop::new(config), tools, recorder)
}

/// Take back the session of work that ended, report a failure, and start
/// what was waiting for the agent: first any messages sent into the turn
/// too late for the model to see, then the queue.
async fn handle_work_done(state: &mut LoopState<'_>, done: WorkDone) {
    let WorkDone {
        agent_id,
        session,
        failure,
        unseen,
        title,
    } = done;
    if let Some(title) = title {
        if let Err(e) = subagents::lock(&state.hub.registry).set_title(agent_id, &title) {
            warn!(%agent_id, error = %e, "Could not save the chat title");
        }
    }
    if !unseen.is_empty() {
        let queue = state.queued.entry(agent_id).or_default();
        for (source, text) in unseen.into_iter().rev() {
            // A user message sent mid-turn is already in the chat.
            queue.push_front(prompt_work(source, text, true));
        }
    }
    state.in_flight = state.in_flight.saturating_sub(1);
    if let Some(session) = session {
        state.idle.insert(agent_id, session);
    }
    let cancelled = state.hub.stop_running(agent_id);
    let status = if failure.is_some() {
        ProcessStatus::Failed
    } else if cancelled {
        ProcessStatus::Killed
    } else {
        ProcessStatus::Exited
    };
    state
        .processes
        .finish(&processes::turn_id(agent_id), status);

    if let Some(problem) = failure {
        let ui = AgentUi::new(state.commands.clone(), agent_id);
        if !local_model_unavailable(state, &ui).await {
            ui.send(UiCommand::ShowError(problem)).await;
            ui.send(UiCommand::Complete).await;
        }
    }

    state.park_finished_spawns().await;
    start_next(state, agent_id).await;
    agents::send_agent_list(state).await;
}

/// Re-probe Ollama when the selected model is local. Returns `true` after
/// telling `ui` why it can't run and ending its turn.
async fn local_model_unavailable(state: &mut LoopState<'_>, ui: &AgentUi) -> bool {
    if local_model_name(state.models.current()).is_none() {
        return false;
    }
    state.models.refresh_local().await;
    let _ = state.commands.send(state.models.ui_command()).await;
    let Some(hint) = state.models.local_hint() else {
        return false;
    };
    info!("Skipping local turn; model server is not ready");
    ui.send(handlers::system_message(hint)).await;
    ui.send(UiCommand::Complete).await;
    true
}

#[cfg(test)]
mod tests;
