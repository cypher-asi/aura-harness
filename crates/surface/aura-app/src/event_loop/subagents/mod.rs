//! Spawning permanent agents from a turn, and the agent state the event
//! loop shares with that spawning.
//!
//! A `task` call names a permanent agent. The dispatcher registers a new
//! instance under the calling agent, runs one turn of it with the
//! definition's rules, tools and model, runs the call's `check` if it has
//! one, and returns a compact result: the instance's reply (capped), the
//! files it changed and how the check went (see [`super::task_check`]).
//! When the user selected gate, Gate picks the model of a subtask nothing
//! pins, and every subtask records its prompt and how it went (see
//! [`super::gate::subtask`]); a routed subtask whose check fails or that
//! errors has its file changes undone and is retried once on the next
//! stronger model (`AgentHub::run_subtask` says which changes are
//! undone). The instance stays in the agents list so the user can open
//! it and keep talking to it.
//!
//! Instances of permanent agents that only read (their tools change no
//! file) run side by side; the others run one at a time per workspace,
//! so their edits never collide. Under Gate the user's own agent leads
//! the turns that ask for a change across several files or for planning:
//! [`LeadProfile::apply`] gives such a turn the lead's prompt and, while
//! delegation is enforced, the lead's tools (see [`aura_gate::lead`]). It
//! does every other turn itself, on the model Gate routes it to.
//!
//! Spawns run inside the parent's tool call, on the parent's turn task, so
//! the event loop keeps running beside them and the user can open a spawn
//! to watch it work. Everything they touch lives in [`AgentHub`] behind
//! short-lived locks that are never held across an `.await`.

use super::agent_events::AgentUi;
use super::agents::{AgentSession, KernelFactory};
use super::approvals::TerminalApprovals;
use super::gate::subtask::{explicit_model, Judgement, SubtaskEnd, SubtaskGate, SubtaskLabel};
use super::handlers::{handle_agent_success, run_agent_turn, system_message, TurnControls};
use super::host::{host_tool_definitions, inbox_tool_definitions, is_inbox_tool, TerminalAgents};
use super::mail::{MailItem, MailSender, Mailbox, TaskMail};
use super::modes::Permissions;
use super::notices::{Notice, NoticeSender};
use super::processes::{task_id, ProcessTable};
use super::record_ui::RecordPanelAgent;
use super::steering::{SteerSource, Steering};
use super::task_check;
use super::tasks::{Spawned, TaskBoard};
use super::trace::Traces;
use super::GateBase;
use crate::agent_definitions::{AgentDefinition, AgentDefinitions, COORDINATOR_TOOLS};
use crate::agent_registry::AgentRegistry;
use crate::event_loop::gate::followup::ended_signal;
use crate::file_checkpoints::AttemptFiles;
use crate::isolation::changes;
use crate::long_run::LongRunSettings;
use crate::model_selection::{routed_effort, ModelLimits};
use crate::project_instructions::ProjectInstructions;
use crate::record_loader::agent_summaries;
use async_trait::async_trait;
use aura_agent::{AgentLoop, AgentLoopConfig};
use aura_agent_kernel::Kernel;
use aura_agent_trace::{RunMeta, RunRecorder, RunSource};
use aura_context_skills::SkillManager;
use aura_core_types::time::now_ms;
use aura_core_types::{
    AgentId, Hash, ModelTokens, OutcomePayload, OutcomeSignal, PromptCategory, RecordEntry,
    RouteLevel, SpawnMode, SubagentDispatchRequest, SubagentExit, SubagentResult, Transaction,
    TransactionType,
};
use aura_exec_tools::agents::task::TASK_TOOL_NAME;
use aura_exec_tools::jobs::{JOB_KILL_TOOL_NAME, JOB_OUTPUT_TOOL_NAME};
use aura_exec_tools::todo::{task_report_line, ASSIGN_TOOL_NAME, TODO_WRITE_TOOL_NAME};
use aura_exec_tools::SubagentDispatchHook;
use aura_gate::compare::{tally, PlanReading};
use aura_gate::followup;
use aura_gate::judge::turn::{shown_to_judge, Verdict};
use aura_gate::lead::lead_tools;
use aura_gate::live::{self, GateLive};
use aura_gate::route::{Candidate, Decision};
use aura_model::{Message, ModelCallPurpose, ThinkingEffort, ToolDefinition};
use aura_store_db::ChannelStore;
use aura_ui::events::{
    AgentDefinitionSummary, AgentSummary, MailView, MessageData, ProcessKind, ProcessStatus,
    ProcessSummary, RecordSummary,
};
use aura_ui::{PermissionMode, UiCommand};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

mod accounting;
mod lists;
mod running;
mod setup;
mod spawn;
mod subtask;
mod task_run;
mod task_tool;
mod task_turn;

use task_run::*;
use task_tool::*;
pub use task_tool::{assign_tool_definition, task_tool_definition};
use task_turn::*;

/// Record entries read per scan when adding up an agent's model calls.
const SCAN_BATCH: usize = 500;

/// How long before its caller gives up on the `task` call a subtask that
/// is waited for stops, so it ends with its run closed and a reason.
const STOP_BEFORE_CALLER: Duration = Duration::from_secs(30);

/// How long before a subtask's time is up it is told to reply with what
/// it has.
const WRAP_UP_BEFORE: Duration = Duration::from_secs(3 * 60);

/// An agent with work in flight.
struct Running {
    kernel: Arc<Kernel>,
    cancel: CancellationToken,
    /// Input for its turn; `None` while it runs a typed shell command or a
    /// task for its parent.
    steering: Option<Steering>,
    /// Subtasks this work dispatched, oldest first
    subtasks: Vec<Attempt>,
    /// Which [`AgentHub::start_running`] call made this entry, so a
    /// [`RunningGuard`] only ever removes the entry it started.
    generation: RunGeneration,
}

/// Identifies one [`AgentHub::start_running`] call. Unique per hub, so an
/// agent that starts running again gets a different one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunGeneration(u64);

/// Ends a spawn's work in flight if the future doing it is dropped before
/// it hands the spawn's session back ([`AgentHub::park`]).
///
/// A waiting spawn runs inside its parent's tool call, and cancelling the
/// parent's turn drops that call's future without ever cancelling the
/// spawn's own token, so nothing after the drop point runs. Without this
/// guard the spawn's `running` entry would stay forever: the agent would
/// read as busy, block archiving and queue every message sent to it.
///
/// Invariants:
/// - The guard removes only the entry its own `start_running` call made
///   (same [`RunGeneration`]); a later turn on the same agent is left
///   alone, however the drops and starts interleave.
/// - Once disarmed (the normal end, where `park` stops the running) its
///   drop does nothing, so cleanup happens exactly once.
/// - It holds the hub weakly: guards are not stored in the hub, but a
///   detached spawn's guard lives on a `tokio::spawn`ed task and must not
///   keep a shut-down hub alive. With the hub gone there is nothing left
///   to clean up.
struct RunningGuard {
    hub: Weak<AgentHub>,
    agent: AgentId,
    generation: RunGeneration,
    armed: bool,
}

impl RunningGuard {
    /// The work ended normally and whoever disarms takes over ending its
    /// running (as [`AgentHub::park`] does).
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Some(hub) = self.hub.upgrade() else {
            return;
        };
        if !hub.stop_running_if(self.agent, self.generation) {
            return;
        }
        info!(agent = %self.agent, "A spawn's work was dropped before it finished");
        hub.processes
            .finish(&task_id(self.agent), ProcessStatus::Killed);
        // Pushed after the entry is gone, so the event loop never finds it
        // still running and skips it. Its session went with the dropped
        // future; the event loop reopens it from the record.
        lock(&hub.abandoned).push(self.agent);
        // Drop can't await to send the agent list; the `spawn_finished`
        // arm of `run_event_loop` sends it after parking, and this wakes it.
        hub.spawn_finished.notify_one();
    }
}

/// A subtask an agent's work dispatched, kept to tell when the same
/// subtask is dispatched again.
struct Attempt {
    /// The permanent agent it went to
    kind: String,
    prompt: String,
    child: AgentId,
    kernel: Arc<Kernel>,
    /// The outcome written when it ended, once it has
    ended: Option<OutcomePayload>,
    /// Whether a later dispatch redid it
    redone: bool,
}

/// A subtask the parent dispatched again: its end outcome is to be
/// followed by a `redone` failure.
struct Redo {
    kernel: Arc<Kernel>,
    child: AgentId,
    ended: OutcomePayload,
}

impl Redo {
    async fn record(self) {
        info!(child = %self.child, "Subtask dispatched again; its first attempt failed");
        live::record(
            &self.kernel,
            self.child,
            &self.ended.with_signal(OutcomeSignal::Redone),
        )
        .await;
    }
}

/// How the subtasks of a `/compare` run are routed, whatever the user
/// selected: the Gate arm's by Gate without exploring, the other arm's
/// on the model it is pinned to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ArmRouting {
    /// Gate routes among `candidates`; `delegate` says whether the run's
    /// lead must delegate (see [`AgentHub::leads`]).
    Gate {
        candidates: Vec<Candidate>,
        delegate: bool,
    },
    /// Every subtask nothing else pins runs on this model.
    Pinned(String),
    /// Every turn and subtask nothing else pins is classified and runs on
    /// the model its difficulty is given, with no exploring and no
    /// escalation.
    ByDifficulty(aura_gate::compare::DifficultyRoutes),
}

/// A running `/compare` run's arm, as its subtasks see it.
#[derive(Debug, Clone)]
struct CompareArmRun {
    routing: ArmRouting,
    /// The arm's own view of Gate (see [`GateLive::snapshot`]), so the
    /// arms never route on each other's observations or draws
    gate: Arc<GateLive>,
}

/// How a turn of one of the user's agents leads under Gate once its
/// prompt asks for work a lead takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LeadProfile {
    /// Whether the lead must delegate, rather than being advised to
    enforced: bool,
}

impl LeadProfile {
    /// Make the turn set up with `config` and `tools` a lead's when it is
    /// `led` (as Gate decides, [`aura_gate::lead::choose_way`]): the
    /// prompt says how to lead and, with delegation enforced, the lead's
    /// tools replace the file tools. Any other turn is left as it is, for
    /// the model Gate routes it to to do the work itself. Returns whether
    /// the turn's calls must stay within the lead's limits
    /// ([`AgentSession::lead`]).
    pub(crate) fn apply(
        self,
        led: bool,
        config: &mut AgentLoopConfig,
        tools: &mut Vec<ToolDefinition>,
    ) -> bool {
        if !led {
            return false;
        }
        config.system_prompt.push_str("\n\n");
        config
            .system_prompt
            .push_str(&aura_context_prompts::system::sections::gate_lead::render(
                self.enforced,
            ));
        if self.enforced {
            *tools = lead_tools(std::mem::take(tools));
        }
        self.enforced
    }
}

/// Whether a user turn coordinates (gets the `<coordinator>` section): the
/// user's own agent, not an instance of a permanent agent, in agent mode,
/// with agents to spawn, and not leading under Gate, whose `<gate_lead>`
/// says how to delegate instead.
fn coordinates(
    definition: Option<&str>,
    mode: PermissionMode,
    leads: bool,
    tools: &[ToolDefinition],
) -> bool {
    definition.is_none()
        && mode == PermissionMode::Agent
        && !leads
        && tools.iter().any(|tool| tool.name == TASK_TOOL_NAME)
}

/// What every turn starts from before a permanent agent's settings apply.
pub struct TurnBase {
    /// The terminal's agent-loop config (selected model, auth, prompt).
    pub config: AgentLoopConfig,
    /// Built-in tools, without `task`.
    pub tools: Vec<ToolDefinition>,
    /// For running a permanent agent on its own model.
    pub limits: ModelLimits,
    /// How Gate classifies and routes the subtasks turns delegate.
    pub gate: GateBase,
}

/// Agent state shared by the event loop and the subagent dispatcher.
pub struct AgentHub {
    pub(crate) registry: Mutex<AgentRegistry>,
    pub(super) definitions: Mutex<AgentDefinitions>,
    pub(super) skills: Mutex<SkillManager>,
    pub(super) base: Mutex<TurnBase>,
    /// Agents with work in flight: turns the user started and spawns
    /// working on a task for their parent.
    running: Mutex<HashMap<AgentId, Running>>,
    /// The [`RunGeneration`] the next `start_running` call gets.
    next_generation: AtomicU64,
    /// Spawns whose work was dropped mid-run, taking their sessions with
    /// it (see [`RunningGuard`]), waiting for the event loop to reopen
    /// them.
    abandoned: Mutex<Vec<AgentId>>,
    /// Sessions of finished spawns, waiting for the event loop to park them.
    finished: Mutex<Vec<AgentSession>>,
    /// Wakes the event loop when a spawn finishes.
    pub(super) spawn_finished: Notify,
    pub(crate) factory: KernelFactory,
    commands: mpsc::Sender<UiCommand>,
    /// Per-turn time and cost limits.
    pub(crate) long_run: LongRunSettings,
    /// Mode and approval level of each agent the user or a parent set them
    /// for
    permissions: Mutex<HashMap<AgentId, Permissions>>,
    /// Mode each agent in plan mode was in before it, which `/build`
    /// returns it to
    before_plan: Mutex<HashMap<AgentId, PermissionMode>>,
    /// Mode and approval level of every other agent
    default_permissions: Permissions,
    /// Asks the user about the agents' tool calls; told each agent's
    /// approval level
    approvals: Option<Arc<TerminalApprovals>>,
    /// `AGENTS.md`, `CLAUDE.md` and rules for each working folder.
    pub(crate) instructions: ProjectInstructions,
    /// What the Processes tab lists.
    pub(crate) processes: Arc<ProcessTable>,
    /// Messages for agents from work that finishes off the event loop.
    pub(crate) notices: NoticeSender,
    notice_rx: Mutex<Option<mpsc::UnboundedReceiver<Notice>>>,
    /// Mail for the INBOX tab, once the event loop opened it
    /// ([`Self::open_mail`]); without it no mail is collected.
    mail: Mutex<Option<MailSender>>,
    pub(crate) agents: Option<TerminalAgents>,
    /// The agent the RECORD panel follows.
    pub(crate) record_panel: RecordPanelAgent,
    /// Every agent's run trace.
    pub(crate) traces: Arc<Traces>,
    /// What Gate has learned, and its latest decisions.
    pub(crate) gate: Arc<GateLive>,
    /// How the subtasks of each running `/compare` run are routed, and
    /// the arm's own view of Gate they are routed on.
    compare_arms: Mutex<HashMap<AgentId, CompareArmRun>>,
    /// Per workspace, the lock that runs write subtasks one at a time
    write_locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    /// Agents doing a write subtask while holding their workspace's lock
    writing: Mutex<HashSet<AgentId>>,
    /// When each agent was last active, for the agent lists
    last_active: Mutex<crate::record_loader::LastActive>,
    /// Channels and their members; `None` where there is no database.
    pub(crate) channels: Option<ChannelStore>,
    /// Every agent in a channel, kept beside the channel store so chat
    /// membership and the channels list don't read the store.
    pub(crate) channel_members: Mutex<HashSet<AgentId>>,
    /// The members of channels without a project: the Projects tree
    /// leaves them out. Members of a channel with a project are listed
    /// under it.
    pub(crate) hidden_members: Mutex<HashSet<AgentId>>,
    /// Replies the channels wait for and what each coordinator has seen.
    pub(crate) channel_chat: super::channels::ChannelChat,
    /// What agents owe each other through their inboxes.
    pub(crate) inbox: super::inbox::Inbox,
    /// The chats of the agents shown last, as read from their records
    pub(super) shown_chats: Mutex<crate::record_loader::ByHead<Vec<MessageData>>>,
    /// The Record panel rows of the agents shown last
    pub(super) shown_records: Mutex<crate::record_loader::ByHead<Vec<RecordSummary>>>,
}

/// Parts of an [`AgentHub`] other than the kernel factory.
pub struct AgentHubParts {
    pub registry: AgentRegistry,
    pub definitions: AgentDefinitions,
    pub skills: SkillManager,
    pub base: TurnBase,
    pub commands: mpsc::Sender<UiCommand>,
    pub(crate) long_run: LongRunSettings,
    /// Mode agents start in
    pub mode: PermissionMode,
    /// Approval level agents start at
    pub approval: aura_ui::ApprovalLevel,
    /// The prompter the agents' kernels ask, if any
    pub approvals: Option<Arc<TerminalApprovals>>,
    pub instructions: ProjectInstructions,
    /// Lists turns, spawns and background work in the Processes tab.
    pub processes: Arc<ProcessTable>,
    /// Runs the agents `spawn_agent` starts; `None` leaves the tool out.
    pub agents: Option<TerminalAgents>,
    /// Shared with the kernels' [`super::UiRecordObserver`].
    pub record_panel: RecordPanelAgent,
    /// Records each agent's runs.
    pub traces: Arc<Traces>,
    /// Gate's table; the kernels' record observer must include it so it
    /// sees every outcome.
    pub gate: Arc<GateLive>,
    /// Channels, in the same database as the registry's agent rows;
    /// `None` leaves channels out.
    pub channels: Option<ChannelStore>,
}

/// What [`AgentHub::send_agent_list`] sends, laid out in one walk of the
/// registry.
pub(super) struct AgentLists {
    /// The projects, the Agents panel rows and the Archive
    commands: [UiCommand; 3],
    /// The agent the rows mark active
    active: AgentId,
    /// Rows of the channel members, left out of the Agents panel
    members: HashMap<AgentId, AgentSummary>,
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
pub(super) mod tests;
