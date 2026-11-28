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
mod bench;
mod bench_source;
pub(crate) mod channels;
mod cloud;
pub(crate) mod command_output;
mod compare;
mod delivery;
pub(crate) mod factory;
pub(crate) mod flow;
mod flow_builder;
pub(crate) mod gate;
pub(crate) mod handlers;
pub(crate) mod host;
pub(crate) mod hud;
pub(crate) mod inbox;
mod lifecycle;
mod log_view;
pub(crate) mod mail;
pub(crate) mod modes;
pub(crate) use modes::Permissions;
pub(crate) mod notices;
pub(crate) mod os_processes;
pub(crate) mod plan_mode;
pub(crate) mod policies;
pub(crate) mod processes;
mod queue;
pub(crate) mod record_ui;
pub(crate) mod repo;
mod rewind;
pub(crate) mod run_command;
mod side_panels;
mod stalls;
pub(crate) mod standing;
mod start_work;
pub(crate) mod steering;
pub(crate) mod subagents;
pub(crate) mod task_check;
pub(crate) mod tasks;
mod titles;
pub(crate) mod trace;
pub(crate) mod turns;
mod ui_events;

pub(crate) use agent_events::forward_agent_events;
pub use agents::{AgentSession, KernelBuilder, KernelFactory};
pub use approvals::TerminalApprovals;
pub use command_output::TerminalCommandOutput;
pub(crate) use delivery::deliver_with_images;
pub use flow::{FlowFeed, FlowInbox};
pub use gate::subtask::GateBase;
pub use host::{TerminalAgents, TerminalChildKernels};
pub use inbox::{AssignTool, HubLink, PeerHooks};
pub use processes::{BackgroundCommands, ProcessTable};
pub use record_ui::{FanoutObserver, RecordPanelAgent, UiRecordObserver};
pub use subagents::{AgentHub, AgentHubParts, TurnBase};
pub use trace::Traces;

use crate::model_selection::ModelSelection;
use aura_agent::AgentLoop;
use aura_agent_kernel::Kernel;
use aura_auth::{AuthManager, PendingLink};
use aura_core_types::{AgentId, Transaction};
use aura_ui::{UiCommand, UiEvent};
use delivery::{deliver, start_next, submit};
use lifecycle::{handle_completion, handle_work_done, shut_down};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use turns::WorkDone;
use ui_events::handle_ui_event;

/// How long quitting waits for cancelled turns to record how they ended.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// How long `SessionTask::finish` waits for the loop to end once told to
/// quit: the [`SHUTDOWN_GRACE`] its turns get to end and save their
/// sessions, plus time to save the Processes tab and close the traces
/// after them. It must stay longer than [`SHUTDOWN_GRACE`], or the turns
/// still running at quit are dropped without saving.
pub(crate) const FINISH_GRACE: Duration = SHUTDOWN_GRACE.saturating_add(Duration::from_secs(3));

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
    /// Feed of the GRAPH tab, when the kernels report to one.
    pub flow: Option<Arc<FlowFeed>>,
    /// Cancelled by `SessionTask::finish` once the UI is gone; the loop
    /// then shuts down exactly as on [`UiEvent::Quit`]. The UI's end of
    /// `events` may still be open then, so its closing can't be the
    /// signal.
    pub quit: CancellationToken,
    /// The main store, for cloud agents' record mirror; `None` leaves
    /// cloud agents off.
    pub cloud_store: Option<Arc<dyn aura_store_db::Store>>,
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
    /// Work waiting for busy agents, oldest first
    queued: queue::WorkQueues,
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
    /// The comparison waiting for `/compare go`, and the one running.
    pub(super) compare: compare::CompareState,
    /// Feed of the GRAPH tab.
    pub(super) flow: Option<Arc<FlowFeed>>,
    /// Cloud agents: the hub, who is moving, cloud turns in flight.
    pub(super) cloud: cloud::CloudState,
    /// Id of the channel on screen, between `UiEvent::OpenChannel` and
    /// `UiEvent::CloseChannel`.
    pub(super) open_channel: Option<String>,
    /// The job filling the panels beside the chat of the agent on screen
    /// (see [`side_panels`])
    side_panels: Option<tokio::task::JoinHandle<()>>,
}

impl LoopState<'_> {
    /// Kernel of `agent_id`, whether it is idle or at work.
    pub(super) fn kernel_for(&self, agent_id: AgentId) -> Option<Arc<Kernel>> {
        // A cloud agent's writer is its machine's kernel; nothing here
        // may write for it (rewinds, sign-in notices) until it is back.
        if self.cloud.node_writes(agent_id) {
            return None;
        }
        self.idle
            .get(&agent_id)
            .map(|s| Arc::clone(&s.kernel))
            .or_else(|| self.hub.running_kernel(agent_id))
    }

    /// Keep the sessions of finished spawns so switching to them resumes
    /// their conversation on the kernel that wrote it, then start any work
    /// that was waiting for them.
    ///
    /// A spawn whose work was dropped mid-run (its parent's turn was
    /// cancelled) lost its session with it. Its session is reopened from
    /// the record here, so an agent with no work in flight has a session
    /// in `idle` again once this has run. If that fails it stays without
    /// one: `activate_agent` and `start_work` open a missing session
    /// lazily, and the other handlers only warn.
    async fn park_finished_spawns(&mut self) {
        for session in self.hub.take_finished() {
            let agent_id = session.agent_id();
            self.idle.insert(agent_id, session);
            start_next(self, agent_id).await;
        }
        for agent_id in self.hub.take_abandoned() {
            if self.idle.contains_key(&agent_id) || self.hub.is_running(agent_id) {
                continue;
            }
            // `open_session` logs why it failed; the queued work below
            // retries the open and reports it in the agent's chat.
            if let Ok(session) = agents::open_session(self, agent_id) {
                self.idle.insert(agent_id, session);
            }
            start_next(self, agent_id).await;
        }
    }

    /// Point the terminal's config at the model the next turn runs on
    /// (Gate's pick when Gate is selected) and publish it as the base for
    /// turns and spawned agents.
    fn sync_turn_base(&mut self) {
        self.models.refresh_route(self.agent_loop.config_mut());
        let config = self.agent_loop.config_mut().clone();
        let mut base = subagents::lock(&self.hub.base);
        base.config = config;
        base.limits = self.models.limits();
        base.gate = GateBase::of(&self.models);
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
    let commands = ctx.commands.clone();
    let publisher = processes::spawn_publisher(Arc::clone(&ctx.hub.processes), commands.clone());
    let (done_tx, done_rx) = mpsc::channel::<WorkDone>(64);
    let (mut state, process_completions, quit) = LoopState::open(ctx, &commands, done_tx);
    let (link_tx, link_rx) = mpsc::channel::<auth::LinkOutcome>(4);
    let mut inbox = Inbox {
        done_rx,
        process_completions,
        notice_rx: state.hub.take_notices(),
        mail_rx: state.hub.open_mail(),
        link_rx,
        link_tx,
        quit,
    };
    factory::service::start(Arc::clone(&state.hub), commands.clone(), inbox.quit.clone());
    state.hub.name_agents();
    let active = state.active;
    state.hub.record_panel.set(active);
    let restored = inbox::restore_saved(&mut state).await;
    send_startup(&mut state, active).await;
    inbox::resume_saved(&mut state, restored).await;

    run_loop(&mut state, &mut inbox).await;
    state.stop_side_panels();
    inbox::flush_saved(&state, &mut inbox.notice_rx).await;
    shut_down(&mut state, &mut inbox.done_rx).await;
    publisher.abort();
    state.processes.save();
    state.hub.traces.close_all();
    Ok(())
}

/// What the loop listens to besides the UI's events.
struct Inbox {
    done_rx: mpsc::Receiver<WorkDone>,
    process_completions: mpsc::Receiver<Transaction>,
    notice_rx: mpsc::UnboundedReceiver<notices::Notice>,
    mail_rx: mpsc::UnboundedReceiver<mail::MailItem>,
    link_rx: mpsc::Receiver<auth::LinkOutcome>,
    link_tx: mpsc::Sender<auth::LinkOutcome>,
    quit: CancellationToken,
}

impl<'a> LoopState<'a> {
    /// The loop's state for `ctx`, with the completions and quit token the
    /// loop listens to.
    fn open(
        ctx: EventLoopContext<'a>,
        commands: &'a mpsc::Sender<UiCommand>,
        work_done: mpsc::Sender<WorkDone>,
    ) -> (Self, mpsc::Receiver<Transaction>, CancellationToken) {
        let EventLoopContext {
            events,
            process_completions,
            commands: _,
            agent_loop,
            session,
            hub,
            memory_manager,
            models,
            auth,
            approvals,
            flow,
            quit,
            cloud_store,
        } = ctx;
        let active = session.agent_id();
        let state = LoopState {
            commands,
            events,
            agent_loop,
            memory_manager,
            models,
            active,
            idle: HashMap::from([(active, session)]),
            queued: queue::WorkQueues::counted(hub.standing.counts()),
            work_done,
            in_flight: 0,
            processes: Arc::clone(&hub.processes),
            hub,
            auth,
            pending_link: None,
            approvals,
            compare: compare::CompareState::in_aura_home(
                &std::env::current_dir()
                    .unwrap_or_default()
                    .join("aura_data"),
            ),
            flow,
            cloud: cloud::CloudState::new(cloud_store),
            open_channel: None,
            side_panels: None,
        };
        (state, process_completions, quit)
    }
}

/// Send the UI what it shows for `active` when the loop starts.
async fn send_startup(state: &mut LoopState<'_>, active: AgentId) {
    agents::send_sandbox_root(state).await;
    hud::send_session_of(state, active).await;
    cloud::start(state).await;
    modes::send_mode(state, active).await;
    policies::send_policies_of(state, active).await;
    modes::warn_if_unconfined(state, active).await;
    hud::send_instructions_of(state, active).await;
    plan_mode::send_plan_of(state, active).await;
    state.hub.traces.send_history(active).await;
    handlers::announce_models(state).await;
}

/// Handle what arrives until the UI quits or goes away.
async fn run_loop(state: &mut LoopState<'_>, inbox: &mut Inbox) {
    let hub = Arc::clone(&state.hub);
    let mut cloud_ended = state.cloud.take_ended();
    let first_sweep = tokio::time::Instant::now() + stalls::SWEEP_EVERY;
    let mut sweeps = tokio::time::interval_at(first_sweep, stalls::SWEEP_EVERY);
    // A loop busy past a sweep runs one late sweep, not a burst of them:
    // strikes count minutes, and a burst would count several at once.
    sweeps.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = sweeps.tick() => {
                stalls::sweep(state).await;
            }
            Some(done) = inbox.done_rx.recv() => {
                handle_work_done(state, done).await;
            }
            Some(ended) = cloud_ended.recv() => {
                cloud::handle_ended(state, ended).await;
            }
            () = hub.spawn_finished.notified() => {
                state.park_finished_spawns().await;
                agents::send_agent_list(state).await;
            }
            Some(completion_tx) = inbox.process_completions.recv() => {
                handle_completion(state, completion_tx).await;
            }
            Some(notice) = inbox.notice_rx.recv() => {
                notices::handle_notice(state, notice).await;
            }
            Some(item) = inbox.mail_rx.recv() => {
                if let Some(sender) = item.title_from {
                    state.hub.title_thread(
                        sender,
                        item.view.thread_id.clone(),
                        item.view.text.clone(),
                    );
                }
                mail::forward(state.commands, item.view).await;
            }
            Some(outcome) = inbox.link_rx.recv() => {
                auth::handle_link_outcome(state, outcome).await;
            }
            () = inbox.quit.cancelled() => break,
            event = state.events.recv() => {
                let Some(event) = event else { break };
                if handle_ui_event(state, event, &inbox.link_tx).await {
                    break;
                }
            }
        }
        // The write goes on without the loop.
        drop(inbox::keep_saved(state));
    }
}

// Names the tests use through `use super::*`.
#[cfg(test)]
use {aura_core_types::RouteReason, aura_model::ToolDefinition, steering::SteerSource};

#[cfg(test)]
mod tests;
