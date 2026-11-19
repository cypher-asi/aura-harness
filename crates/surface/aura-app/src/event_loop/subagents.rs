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
//! [`super::gate_subtask`]); a routed subtask whose check fails or that
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
//! delegation is enforced, the lead's tools (see [`super::lead`]). It
//! does every other turn itself, on the model Gate routes it to.
//!
//! Spawns run inside the parent's tool call, on the parent's turn task, so
//! the event loop keeps running beside them and the user can open a spawn
//! to watch it work. Everything they touch lives in [`AgentHub`] behind
//! short-lived locks that are never held across an `.await`.

use super::agent_events::AgentUi;
use super::agents::{AgentSession, KernelFactory};
use super::approvals::TerminalApprovals;
use super::gate_live::GateLive;
use super::gate_record;
use super::gate_subtask::{explicit_model, GateBase, SubtaskGate, SubtaskLabel};
use super::handlers::{handle_agent_success, run_agent_turn, system_message};
use super::host::{agent_tool_definitions, TerminalAgents};
use super::lead::lead_tools;
use super::modes::Permissions;
use super::notices::{Notice, NoticeSender};
use super::processes::{now_ms, task_id, ProcessTable};
use super::record_ui::RecordPanelAgent;
use super::steering::Steering;
use super::task_check;
use super::tasks::{Spawned, TaskBoard};
use super::trace::Traces;
use super::turns::ended_signal;
use crate::agent_definitions::{AgentDefinition, AgentDefinitions};
use crate::agent_registry::AgentRegistry;
use crate::file_checkpoints::AttemptFiles;
use crate::isolation::changes;
use crate::long_run::LongRunSettings;
use crate::model_selection::ModelLimits;
use crate::project_instructions::ProjectInstructions;
use crate::record_loader::agent_summaries;
use async_trait::async_trait;
use aura_agent::{AgentLoop, AgentLoopConfig};
use aura_agent_gate::compare::{tally, PlanReading};
use aura_agent_gate::turn_judge::{Failed, Verdict};
use aura_agent_gate::{Candidate, Decision};
use aura_agent_kernel::Kernel;
use aura_agent_trace::{RunMeta, RunRecorder, RunSource};
use aura_context_skills::SkillManager;
use aura_core_types::{
    AgentId, Hash, ModelTokens, OutcomePayload, OutcomeSignal, PromptCategory, RecordEntry,
    RouteLevel, SpawnMode, SubagentDispatchRequest, SubagentExit, SubagentResult, Transaction,
    TransactionType,
};
use aura_exec_tools::agents::task::TASK_TOOL_NAME;
use aura_exec_tools::jobs::{JOB_KILL_TOOL_NAME, JOB_OUTPUT_TOOL_NAME};
use aura_exec_tools::todo::{task_report_line, TODO_WRITE_TOOL_NAME};
use aura_exec_tools::SubagentDispatchHook;
use aura_model::{Message, ToolDefinition};
use aura_ui_terminal::events::{
    AgentDefinitionSummary, AgentSummary, ProcessKind, ProcessStatus, ProcessSummary,
};
use aura_ui_terminal::{PermissionMode, UiCommand};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Instant;
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Record entries read per scan when adding up an agent's model calls.
const SCAN_BATCH: usize = 500;

/// An agent with work in flight.
struct Running {
    kernel: Arc<Kernel>,
    cancel: CancellationToken,
    /// Input for its turn; `None` while it runs a typed shell command or a
    /// task for its parent.
    steering: Option<Steering>,
    /// Subtasks this work dispatched, oldest first
    subtasks: Vec<Attempt>,
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
        gate_record::record(
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
}

/// How a turn of one of the user's agents leads under Gate once its
/// prompt asks for work a lead takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LeadProfile {
    /// Whether the lead must delegate, rather than being advised to
    enforced: bool,
}

impl LeadProfile {
    /// Make the turn set up with `config` and `tools` a lead's when its
    /// prompt asks for `category` ([`aura_agent_gate::lead::leads`]): the
    /// prompt says how to lead and, with delegation enforced, the lead's
    /// tools replace the file tools. Any other turn is left as it is, for
    /// the model Gate routes it to to do the work itself. Returns whether
    /// the turn's calls must stay within the lead's limits
    /// ([`AgentSession::lead`]).
    pub(crate) fn apply(
        self,
        category: PromptCategory,
        config: &mut AgentLoopConfig,
        tools: &mut Vec<ToolDefinition>,
    ) -> bool {
        if !aura_agent_gate::lead::leads(category) {
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
    pub(crate) agents: Option<TerminalAgents>,
    /// The agent the RECORD panel follows.
    pub(crate) record_panel: RecordPanelAgent,
    /// Every agent's run trace.
    pub(crate) traces: Arc<Traces>,
    /// What Gate has learned, and its latest decisions.
    pub(crate) gate: Arc<GateLive>,
    /// How the subtasks of each running `/compare` run are routed.
    compare_arms: Mutex<HashMap<AgentId, ArmRouting>>,
    /// Per workspace, the lock that runs write subtasks one at a time
    write_locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    /// Agents doing a write subtask while holding their workspace's lock
    writing: Mutex<HashSet<AgentId>>,
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
    pub approval: aura_ui_terminal::ApprovalLevel,
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
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl AgentHub {
    /// Build the hub. `factory` receives the dispatcher that kernels must
    /// install so their `task` calls spawn permanent agents.
    pub fn new(
        parts: AgentHubParts,
        factory: impl FnOnce(Arc<dyn SubagentDispatchHook>) -> KernelFactory,
    ) -> Arc<Self> {
        let (notices, notice_rx) = mpsc::unbounded_channel();
        let default_permissions = Permissions::new(parts.mode, parts.approval);
        if let Some(approvals) = &parts.approvals {
            approvals.set_default_level(parts.approval);
        }
        let tasks = Arc::new(TaskBoard::new(
            parts.commands.clone(),
            parts.record_panel.clone(),
        ));
        Arc::new_cyclic(|hub: &Weak<Self>| {
            let dispatcher: Arc<dyn SubagentDispatchHook> =
                Arc::new(TerminalSubagentDispatcher { hub: hub.clone() });
            let mut factory = factory(dispatcher);
            factory.tasks = tasks;
            Self {
                registry: Mutex::new(parts.registry),
                definitions: Mutex::new(parts.definitions),
                skills: Mutex::new(parts.skills),
                base: Mutex::new(parts.base),
                running: Mutex::new(HashMap::new()),
                finished: Mutex::new(Vec::new()),
                spawn_finished: Notify::new(),
                factory,
                commands: parts.commands,
                long_run: parts.long_run,
                permissions: Mutex::new(HashMap::new()),
                before_plan: Mutex::new(HashMap::new()),
                default_permissions,
                approvals: parts.approvals,
                instructions: parts.instructions,
                processes: parts.processes,
                notices,
                notice_rx: Mutex::new(Some(notice_rx)),
                agents: parts.agents,
                record_panel: parts.record_panel,
                traces: parts.traces,
                gate: parts.gate,
                compare_arms: Mutex::new(HashMap::new()),
                write_locks: Mutex::new(HashMap::new()),
                writing: Mutex::new(HashSet::new()),
            }
        })
    }

    /// Route the subtasks of the `/compare` run `agent` (and theirs) as
    /// `routing` says, until [`Self::end_compare_run`].
    pub(crate) fn begin_compare_run(&self, agent: AgentId, routing: ArmRouting) {
        lock(&self.compare_arms).insert(agent, routing);
    }

    /// The `/compare` run `agent` ended.
    pub(crate) fn end_compare_run(&self, agent: AgentId) {
        lock(&self.compare_arms).remove(&agent);
    }

    /// How the subtasks of `agent` are routed when it works for a
    /// `/compare` run, directly or through the agents between.
    fn compare_routing(&self, agent: AgentId) -> Option<ArmRouting> {
        let ancestry = lock(&self.registry).ancestry(agent);
        let arms = lock(&self.compare_arms);
        ancestry.iter().find_map(|id| arms.get(id).cloned())
    }

    /// Hand `session`, whose work ended, to the event loop to keep, and
    /// note that it stopped running.
    pub(crate) fn park(&self, session: AgentSession) {
        let agent_id = session.agent_id();
        // Parked before it stops running, so the event loop never sees the
        // agent with neither a session nor work in flight.
        lock(&self.finished).push(session);
        self.stop_running(agent_id);
        self.spawn_finished.notify_one();
    }

    /// Work mode of `agent_id`.
    pub fn mode_of(&self, agent_id: AgentId) -> PermissionMode {
        self.permissions_of(agent_id).mode
    }

    /// Work mode and approval level of `agent_id`.
    pub(crate) fn permissions_of(&self, agent_id: AgentId) -> Permissions {
        lock(&self.permissions)
            .get(&agent_id)
            .copied()
            .unwrap_or(self.default_permissions)
    }

    /// Note that `agent_id` works under `permissions` from now on.
    pub(super) fn set_permissions(&self, agent_id: AgentId, permissions: Permissions) {
        let from = lock(&self.permissions).insert(agent_id, permissions);
        let from = from.unwrap_or(self.default_permissions).mode;
        if permissions.mode == PermissionMode::Plan && from != PermissionMode::Plan {
            lock(&self.before_plan).insert(agent_id, from);
        }
        if let Some(approvals) = &self.approvals {
            approvals.set_level(agent_id, permissions.approval);
        }
    }

    /// The mode `/plan` returns `agent_id` to: the one it had before plan
    /// mode, or `agent` when the session started there.
    pub(super) fn exit_plan_mode(&self, agent_id: AgentId) -> PermissionMode {
        lock(&self.before_plan)
            .get(&agent_id)
            .copied()
            .filter(|mode| *mode != PermissionMode::Plan)
            .unwrap_or(PermissionMode::Agent)
    }

    /// The mode `/build` puts `agent_id` in: its current one when that
    /// lets it edit, else the one it had before plan mode, else
    /// `agent`.
    pub(super) fn build_mode(&self, agent_id: AgentId) -> PermissionMode {
        let mode = self.mode_of(agent_id);
        if mode.agent_mode() == aura_core_types::AgentMode::Agent {
            return mode;
        }
        lock(&self.before_plan)
            .get(&agent_id)
            .copied()
            .filter(|before| before.agent_mode() == aura_core_types::AgentMode::Agent)
            .unwrap_or(PermissionMode::Agent)
    }

    /// Names of every tool a turn may offer, for the mode tables.
    pub(crate) fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = lock(&self.base)
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        names.push(TASK_TOOL_NAME.to_string());
        if self.agents.is_some() {
            names.extend(agent_tool_definitions().into_iter().map(|tool| tool.name));
        }
        names
    }

    /// Config and tools for a turn of an agent working in `workspace`,
    /// applying the permanent agent `definition` when it names one.
    pub fn turn(
        &self,
        definition: Option<&str>,
        workspace: &Path,
    ) -> (AgentLoopConfig, Vec<ToolDefinition>) {
        let instructions = self.instructions.for_workspace(workspace);
        let (mut config, tools) = self.definition_turn(definition, &instructions.text);
        self.long_run.apply(&mut config);
        (config, tools)
    }

    /// Whether the agent `agent_id`, an instance of the permanent agent
    /// `definition` if any, leads under Gate the turns that call for a
    /// lead ([`aura_agent_gate::lead::leads`]): `Some(true)` with
    /// delegation enforced, `Some(false)` with it only recommended
    /// (`AURA_GATE_DELEGATE=off`, or the `gate:nodelegate` arm of
    /// `/compare`), `None` when it never leads. Only the user's own
    /// agents lead, and only when Gate routes their work: never an
    /// instance of a permanent agent, a pinned main model or a pinned
    /// `/compare` arm.
    pub(crate) fn leads(&self, agent_id: AgentId, definition: Option<&str>) -> Option<bool> {
        if definition.is_some() {
            return None;
        }
        match self.compare_routing(agent_id) {
            Some(ArmRouting::Gate { delegate, .. }) => Some(delegate),
            Some(ArmRouting::Pinned(_)) => None,
            None => lock(&self.base)
                .gate
                .candidates
                .is_some()
                .then(|| self.gate.settings().delegate),
        }
    }

    /// [`Self::agent_turn`] for a turn `session` runs for the user (or a
    /// `/compare` run), and how the turn leads when its prompt turns out
    /// to ask for work a lead takes ([`LeadProfile::apply`]); `None` when
    /// it never leads (see [`Self::leads`]). `session` starts the turn
    /// without the lead's limits.
    pub(crate) fn user_turn(
        &self,
        session: &mut AgentSession,
    ) -> (AgentLoopConfig, Vec<ToolDefinition>, Option<LeadProfile>) {
        let workspace = session.kernel.agent_workspace();
        let agent_id = session.agent_id();
        let definition = session.definition.clone();
        let (mut config, tools) = self.agent_turn(agent_id, definition.as_deref(), &workspace);
        session.lead = false;
        let lead = self
            .leads(agent_id, definition.as_deref())
            .map(|enforced| LeadProfile { enforced });
        if coordinates(
            definition.as_deref(),
            self.mode_of(agent_id),
            lead.is_some(),
            &tools,
        ) {
            config.system_prompt.push_str("\n\n");
            config
                .system_prompt
                .push_str(&aura_context_prompts::system::sections::coordinator::render());
        }
        (config, tools, lead)
    }

    /// [`Self::turn`] for `agent_id`, with the plan-mode instructions when
    /// the agent is in plan mode.
    pub(crate) fn agent_turn(
        &self,
        agent_id: AgentId,
        definition: Option<&str>,
        workspace: &Path,
    ) -> (AgentLoopConfig, Vec<ToolDefinition>) {
        let (mut config, tools) = self.turn(definition, workspace);
        if self.mode_of(agent_id) == PermissionMode::Plan {
            config.system_prompt.push_str("\n\n");
            config
                .system_prompt
                .push_str(&aura_context_prompts::system::sections::plan_mode::render());
        }
        (config, tools)
    }

    /// Show `agent_id` the instruction files its turns load from
    /// `workspace`.
    pub(super) async fn send_instructions(&self, agent_id: AgentId, workspace: &Path) {
        let files = self.instructions.for_workspace(workspace).files.clone();
        AgentUi::new(self.commands.clone(), agent_id)
            .send(UiCommand::SetInstructions(files))
            .await;
    }

    fn definition_turn(
        &self,
        definition: Option<&str>,
        instructions: &str,
    ) -> (AgentLoopConfig, Vec<ToolDefinition>) {
        let base = lock(&self.base);
        let definitions = lock(&self.definitions);
        let mut config = base.config.clone();
        // Right after the base prompt, ahead of anything that varies
        // between agents or turns, so the cached prompt prefix covers it.
        if !instructions.is_empty() {
            config.system_prompt.push_str("\n\n");
            config.system_prompt.push_str(instructions);
        }
        let mut tools = base.tools.clone();
        if let Some(task) = task_tool_definition(&definitions) {
            tools.push(task);
        }
        if self.agents.is_some() {
            tools.extend(agent_tool_definitions());
        }
        let Some(name) = definition else {
            return (config, tools);
        };
        let Some(definition) = definitions.get(name) else {
            warn!(
                definition = name,
                "Permanent agent no longer exists; using defaults"
            );
            return (config, tools);
        };
        let filtered = definition.filter_tools(&tools);
        // Planning and job tools only touch the agent's own state, so a
        // permanent agent's tool list never needs to name them.
        let keep_harness_tool = |tool: &ToolDefinition| {
            tool.name == TODO_WRITE_TOOL_NAME
                || ((tool.name == JOB_OUTPUT_TOOL_NAME || tool.name == JOB_KILL_TOOL_NAME)
                    && filtered.iter().any(|t| t.name == "run_command"))
        };
        let harness: Vec<ToolDefinition> = tools
            .iter()
            .filter(|tool| keep_harness_tool(tool) && !filtered.iter().any(|t| t.name == tool.name))
            .cloned()
            .collect();
        tools = filtered;
        tools.extend(harness);
        definition.extend_system_prompt(&mut config.system_prompt, &lock(&self.skills));
        if let Some(model) = &definition.model {
            base.limits.configure(&mut config, model);
        }
        (config, tools)
    }

    /// Re-read the permanent agents, returning files that could not be used.
    pub fn reload_definitions(&self) -> Vec<String> {
        lock(&self.skills).reload();
        lock(&self.definitions).reload()
    }

    /// Rows for the roster (the agents panel's Agents tab).
    pub fn definition_summaries(&self) -> Vec<AgentDefinitionSummary> {
        lock(&self.definitions)
            .all()
            .iter()
            .map(|d| AgentDefinitionSummary {
                name: d.name.clone(),
                description: d.description.clone(),
            })
            .collect()
    }

    /// Rows for the Agents panel.
    pub fn agent_summaries(&self) -> Vec<AgentSummary> {
        let registry = lock(&self.registry);
        let running: HashSet<AgentId> = lock(&self.running).keys().copied().collect();
        agent_summaries(
            &registry,
            self.factory.store.as_ref(),
            registry.active().agent_id(),
            &running,
        )
    }

    /// Give `agent` the chat title `title` and show it in the Agents panel
    /// right away, unless the agent got a name or title in the meantime.
    pub(super) async fn apply_title(&self, agent: AgentId, title: &str) {
        let applied = {
            let mut registry = lock(&self.registry);
            let wanted = registry
                .find(agent)
                .is_some_and(crate::agent_registry::RegisteredAgent::needs_title);
            wanted
                && match registry.set_title(agent, title) {
                    Ok(()) => true,
                    Err(e) => {
                        warn!(%agent, error = %e, "Could not save the chat title");
                        false
                    }
                }
        };
        if applied {
            self.send_agent_list().await;
        }
    }

    /// Send the Agents panel rows and the active agent to the UI.
    pub async fn send_agent_list(&self) {
        self.name_agents();
        let summaries = self.agent_summaries();
        let archived = crate::record_loader::archived_summaries(
            &lock(&self.registry),
            self.factory.store.as_ref(),
        );
        let active = lock(&self.registry).active().agent_id();
        let _ = self.commands.send(UiCommand::SetAgents(summaries)).await;
        let _ = self
            .commands
            .send(UiCommand::SetArchivedAgents(archived))
            .await;
        let _ = self
            .commands
            .send(UiCommand::SetActiveAgent(active.to_hex()))
            .await;
    }

    /// Let `agent`, about to run a turn on `config` in `folder`, spawn
    /// background agents in its own mode, and list them in the Processes
    /// tab.
    pub(crate) fn register_root(
        &self,
        agent: AgentId,
        name: &str,
        folder: PathBuf,
        config: &AgentLoopConfig,
    ) {
        if let Some(agents) = &self.agents {
            let mode = self.mode_of(agent).agent_mode();
            agents.register(agent, name, folder, config, mode, self.notices.clone());
            self.processes.add_root(agent);
        }
    }

    /// The receiving end of [`Self::notices`], for the event loop. Later
    /// calls get a receiver that never yields.
    pub(crate) fn take_notices(&self) -> mpsc::UnboundedReceiver<Notice> {
        lock(&self.notice_rx)
            .take()
            .unwrap_or_else(|| mpsc::unbounded_channel().1)
    }

    /// Tell the Processes tab what every agent is called.
    pub(crate) fn name_agents(&self) {
        for agent in lock(&self.registry).agents() {
            self.processes
                .name_agent(agent.agent_id(), agent.display_name());
        }
    }

    /// Send the roster rows to the UI.
    pub async fn send_definitions(&self) {
        let summaries = self.definition_summaries();
        let _ = self
            .commands
            .send(UiCommand::SetAgentDefinitions(summaries))
            .await;
    }

    /// Sessions of spawns that finished since the last call.
    pub fn take_finished(&self) -> Vec<AgentSession> {
        std::mem::take(&mut *lock(&self.finished))
    }

    /// Note that `agent_id` started work on `kernel` that `cancel` stops.
    /// `steering` takes messages for it while it runs.
    pub(crate) fn start_running(
        &self,
        agent_id: AgentId,
        kernel: Arc<Kernel>,
        cancel: CancellationToken,
        steering: Option<Steering>,
    ) {
        lock(&self.running).insert(
            agent_id,
            Running {
                kernel,
                cancel,
                steering,
                subtasks: Vec::new(),
            },
        );
    }

    /// Note that the work `parent` is doing dispatched `attempt`. Returns
    /// the subtasks of the same kind and prompt it dispatched before that
    /// have ended and not been redone yet; those still running are marked
    /// to be redone when they end. Nothing is kept for an agent with no
    /// work in flight.
    fn begin_attempt(&self, parent: AgentId, attempt: Attempt) -> Vec<Redo> {
        let mut running = lock(&self.running);
        let Some(work) = running.get_mut(&parent) else {
            return Vec::new();
        };
        let mut redos = Vec::new();
        for earlier in &mut work.subtasks {
            if earlier.redone
                || earlier.kind != attempt.kind
                || !aura_agent_gate::outcome::is_repeat(&earlier.prompt, &attempt.prompt)
            {
                continue;
            }
            earlier.redone = true;
            if let Some(ended) = &earlier.ended {
                redos.push(Redo {
                    kernel: Arc::clone(&earlier.kernel),
                    child: earlier.child,
                    ended: ended.clone(),
                });
            }
        }
        work.subtasks.push(attempt);
        redos
    }

    /// Note that `parent`'s subtask on `child` ended with `ended`. Returns
    /// it as a [`Redo`] when the parent already dispatched it again.
    fn end_attempt(
        &self,
        parent: AgentId,
        child: AgentId,
        ended: Option<OutcomePayload>,
    ) -> Option<Redo> {
        let mut running = lock(&self.running);
        let attempt = running
            .get_mut(&parent)?
            .subtasks
            .iter_mut()
            .find(|attempt| attempt.child == child)?;
        attempt.ended.clone_from(&ended);
        let ended = ended.filter(|_| attempt.redone)?;
        Some(Redo {
            kernel: Arc::clone(&attempt.kernel),
            child,
            ended,
        })
    }

    /// Input for the turn `agent_id` is running, if it is running one.
    pub(super) fn steering(&self, agent_id: AgentId) -> Option<Steering> {
        lock(&self.running)
            .get(&agent_id)
            .and_then(|running| running.steering.clone())
    }

    /// Note that `agent_id` has no work in flight any more. Returns
    /// whether that work was cancelled.
    pub(crate) fn stop_running(&self, agent_id: AgentId) -> bool {
        self.gate.forget_model(agent_id);
        lock(&self.running)
            .remove(&agent_id)
            .is_some_and(|running| running.cancel.is_cancelled())
    }

    /// Whether `agent_id` has work in flight.
    pub(crate) fn is_running(&self, agent_id: AgentId) -> bool {
        lock(&self.running).contains_key(&agent_id)
    }

    /// Kernel of `agent_id` while it has work in flight.
    pub(super) fn running_kernel(&self, agent_id: AgentId) -> Option<Arc<Kernel>> {
        lock(&self.running)
            .get(&agent_id)
            .map(|running| Arc::clone(&running.kernel))
    }

    /// Stop the work of `agent_id`. Returns whether it had any.
    pub(super) fn cancel(&self, agent_id: AgentId) -> bool {
        lock(&self.running)
            .get(&agent_id)
            .map(|running| running.cancel.cancel())
            .is_some()
    }

    /// Stop the work of every agent.
    pub(super) fn cancel_all(&self) {
        for running in lock(&self.running).values() {
            running.cancel.cancel();
        }
    }

    async fn spawn(
        self: &Arc<Self>,
        request: SubagentDispatchRequest,
    ) -> Result<SubagentResult, String> {
        let definition = lock(&self.definitions).get(&request.subagent_type).cloned();
        let Some(definition) = definition else {
            let available: Vec<String> = lock(&self.definitions)
                .all()
                .iter()
                .map(|d| d.name.clone())
                .collect();
            return Ok(SubagentResult::rejected(format!(
                "task: no permanent agent named `{}`; available: {}",
                request.subagent_type,
                available.join(", ")
            )));
        };

        let first = self
            .open_task(&request, &definition, request.prompt.clone(), None)
            .await?;
        let parent = request.parent_agent_id;
        let child_id = first.session.agent_id();
        let redos = self.begin_attempt(
            parent,
            Attempt {
                kind: definition.name.clone(),
                prompt: request.prompt.clone(),
                child: child_id,
                kernel: Arc::clone(&first.session.kernel),
                ended: None,
                redone: false,
            },
        );
        for redo in redos {
            redo.record().await;
        }
        let name = first.name.clone();
        let detached = request.spawn_mode == Some(SpawnMode::Detached);
        self.record_spawn(&request, &definition, child_id, &name)
            .await;
        self.factory
            .tasks
            .begin_spawn(Spawned {
                parent,
                child: child_id,
                workspace: first.session.kernel.agent_workspace(),
                name: &name,
                assignee: &definition.name,
                prompt: &request.prompt,
                tool_use_id: request.tool_call_id.clone(),
                todo_id: request.todo_id.clone(),
            })
            .await;
        let todo = request.todo_id.clone();
        let subtask = Subtask {
            request,
            definition,
            first,
        };

        if detached {
            info!(child = %name, "Running permanent agent in the background");
            let hub = Arc::clone(self);
            let shown = name.clone();
            tokio::spawn(async move {
                let report = hub.run_subtask(subtask).await;
                hub.factory
                    .tasks
                    .finish_spawn(child_id, report.outcome())
                    .await;
                let _ = hub.notices.send(task_notice(
                    parent,
                    &shown,
                    todo.as_deref(),
                    report.outcome(),
                ));
            });
            return Ok(SubagentResult::completed(
                child_id,
                format!(
                    "{name} is working on this in the background. Its reply will arrive \
                     as a message when it finishes; carry on meanwhile."
                ),
            ));
        }

        let report = self.run_subtask(subtask).await;
        self.factory
            .tasks
            .finish_spawn(child_id, report.outcome())
            .await;
        if let Err(reason) = &report.last().reply {
            warn!(child = %report.last().name, %reason, "Permanent agent failed");
        }
        Ok(report.result())
    }

    /// Write on the parent's record that `request` went to `child`, an
    /// instance of `definition` shown as `name`: which `task` call it
    /// answers, and the todo it does. A failure is logged; the child's
    /// own record still says who spawned it.
    async fn record_spawn(
        &self,
        request: &SubagentDispatchRequest,
        definition: &AgentDefinition,
        child: AgentId,
        name: &str,
    ) {
        let parent = request.parent_agent_id;
        let Some(kernel) = self.running_kernel(parent) else {
            return;
        };
        let mode = if request.spawn_mode == Some(SpawnMode::Detached) {
            "detached"
        } else {
            "wait"
        };
        let payload = serde_json::json!({
            "parent_agent_id": parent.to_hex(),
            "child_agent_id": child.to_hex(),
            "spawn_mode": mode,
            "subagent_type": definition.name,
            "child_name": name,
            "tool_use_id": request.tool_call_id,
            "todo_id": request.todo_id,
        });
        let tx = Transaction::new_chained(
            parent,
            TransactionType::SubagentSpawn,
            payload.to_string().into_bytes(),
            None,
        );
        if let Err(e) = kernel.process_direct(tx).await {
            warn!(%parent, %child, error = %e, "Could not record the spawn");
        }
    }

    /// Register and open an instance of `definition` for `request`, set up
    /// to be asked `prompt`; `escalation` sends it where a failed first
    /// attempt is retried.
    async fn open_task(
        &self,
        request: &SubagentDispatchRequest,
        definition: &AgentDefinition,
        prompt: String,
        escalation: Option<(Decision, PromptCategory)>,
    ) -> Result<TaskRun, String> {
        let (instance, folder) = {
            let mut registry = lock(&self.registry);
            let parent = registry
                .find(request.parent_agent_id)
                .map(|a| a.zns_id.clone());
            let instance = registry
                .create_instance(&definition.name, parent)
                .map_err(|e| format!("registering {} instance: {e:#}", definition.name))?;
            let folder = registry.project_folder(instance.agent_id());
            (instance, folder)
        };
        let session = self
            .factory
            .open(&instance, request.parent_chain.clone(), folder)
            .map_err(|e| format!("opening {}: {e:#}", instance.name))?;
        let child_id = session.agent_id();
        info!(child = %instance.name, definition = %definition.name, "Spawning permanent agent");
        // A spawn asks no less than the agent that started it.
        let permissions = self.permissions_of(request.parent_agent_id);
        self.set_permissions(child_id, permissions);
        super::modes::apply(&session.kernel, permissions, &self.tool_names());

        let cancel = CancellationToken::new();
        // Messages for a spawn wait for its task to end: steering it
        // mid-task would change the work its parent is waiting on.
        self.start_running(child_id, Arc::clone(&session.kernel), cancel.clone(), None);
        self.send_agent_list().await;

        let workspace = session.kernel.agent_workspace();
        self.send_instructions(child_id, &workspace).await;
        let (mut config, tools) = self.agent_turn(child_id, Some(&definition.name), &workspace);
        config.initial_todos.clone_from(&session.todos);
        // Observers report on the agent they were installed for, not this one.
        config.observers.clear();
        if let Some(addendum) = request
            .system_prompt_addendum
            .as_deref()
            .filter(|a| !a.trim().is_empty())
        {
            config.system_prompt.push_str("\n\n");
            config.system_prompt.push_str(addendum);
        }
        let mut trace = RunMeta::new(&config.model, RunSource::Tui);
        trace.workspace = workspace.display().to_string();
        trace.mode.clone_from(&definition.name);
        trace.spawned_by = self.traces.run_id_of(request.parent_agent_id);
        let mut gate = self.subtask_gate(request, definition, &mut config);
        gate.escalation = escalation;
        let mut summary = task_summary(
            request,
            &prompt,
            &instance.name,
            &definition.name,
            child_id,
            &config.model,
        );
        if request.spawn_mode == Some(SpawnMode::Detached) {
            summary.kind = ProcessKind::Agent;
        }
        Ok(TaskRun {
            session,
            config,
            tools,
            prompt,
            cancel,
            gate,
            summary,
            name: instance.name.clone(),
            parent: request.parent_agent_id,
            check: request.check.clone(),
            trace,
        })
    }

    /// Run `subtask` to its end: wait for its workspace when it writes
    /// there, run its first attempt and, when Gate routed it and the
    /// attempt failed, put back the files it changed and retry it once on
    /// a stronger model.
    ///
    /// The files its file tools changed are always put back. Changes by
    /// its shell commands are too when the workspace is in a git
    /// repository and the attempt had the workspace to itself: it took
    /// the write lock (so it is no read-only or nested subtask) and its
    /// caller waits for it rather than working on beside it.
    async fn run_subtask(&self, subtask: Subtask) -> TaskReport {
        let Subtask {
            request,
            definition,
            first,
        } = subtask;
        let workspace = first.session.kernel.agent_workspace();
        let mut slot = if reads_only(&definition) {
            None
        } else {
            Some(
                self.write_slot(
                    &request,
                    workspace.clone(),
                    &first.cancel,
                    &AgentUi::new(self.commands.clone(), first.session.agent_id()),
                )
                .await,
            )
        };
        if let Some(slot) = &mut slot {
            slot.hold(first.session.agent_id());
        }
        let candidates = first.gate.candidates.clone();
        let alone = slot.as_ref().is_some_and(WriteSlot::exclusive)
            && request.spawn_mode != Some(SpawnMode::Detached);
        let snapshot = if alone && candidates.is_some() {
            snapshot_workspace(workspace).await
        } else {
            None
        };
        let first = self.finish_task(first).await;
        let Some(mut why) = first.attempted().failure().filter(|_| !first.cancelled) else {
            return TaskReport { first, retry: None };
        };
        match &first.judged {
            // Not the model's doing: a stronger model would fail the
            // same way.
            Some(verdict) if !verdict.at_fault => {
                info!(child = %first.name, "The judge excused the subtask's failure; not retried");
                return TaskReport { first, retry: None };
            }
            Some(verdict) if !verdict.reasons.trim().is_empty() => {
                why.push_str("\nA reviewer of that attempt found: ");
                why.push_str(verdict.reasons.trim());
            }
            _ => {}
        }
        let Some(candidates) = candidates else {
            return TaskReport { first, retry: None };
        };
        let category = first.category.unwrap_or(PromptCategory::Unclassified);
        let escalated =
            self.gate
                .escalate(RouteLevel::Subtask, category, &first.model, &candidates);
        let Some(decision) = escalated else {
            info!(child = %first.name, "No stronger model to retry the subtask on");
            return TaskReport { first, retry: None };
        };
        if let Some(files) = &first.files_before {
            let report = files.restore();
            for (path, problem) in &report.failed {
                warn!(path = %path.display(), %problem, "Could not undo a failed subtask's change");
            }
        }
        if let Some(snapshot) = snapshot {
            restore_workspace(snapshot).await;
        }
        let prompt = task_check::retry_prompt(&request.prompt, &first.model, &why);
        let retry = match self
            .open_task(&request, &definition, prompt, Some((decision, category)))
            .await
        {
            Ok(retry) => retry,
            Err(problem) => {
                warn!(%problem, "Could not retry the subtask");
                return TaskReport { first, retry: None };
            }
        };
        if let Some(slot) = &mut slot {
            slot.hold(retry.session.agent_id());
        }
        let retry = self.finish_task(retry).await;
        TaskReport {
            first,
            retry: Some((retry, why)),
        }
    }

    /// The turn of a write subtask for `request` in `workspace`: write
    /// subtasks in one workspace run one at a time, except those
    /// dispatched by one that holds it. Cancelling stops the wait.
    async fn write_slot(
        &self,
        request: &SubagentDispatchRequest,
        workspace: PathBuf,
        cancel: &CancellationToken,
        ui: &AgentUi,
    ) -> WriteSlot<'_> {
        let nested = {
            let writing = lock(&self.writing);
            std::iter::once(&request.parent_agent_id)
                .chain(&request.parent_chain)
                .any(|agent| writing.contains(agent))
        };
        let guard = if nested {
            None
        } else {
            let mutex = Arc::clone(lock(&self.write_locks).entry(workspace).or_default());
            if let Ok(guard) = Arc::clone(&mutex).try_lock_owned() {
                Some(guard)
            } else {
                // The agent is listed as working from here on, but its turn
                // (and so its chat and record) starts only once it has the
                // workspace. Say so rather than show an empty chat.
                ui.send(system_message(
                    "Queued: another agent is editing this workspace. \
                     This agent starts when it finishes."
                        .to_string(),
                ))
                .await;
                tokio::select! {
                    guard = mutex.lock_owned() => Some(guard),
                    () = cancel.cancelled() => None,
                }
            }
        };
        WriteSlot {
            hub: self,
            guard,
            holders: Vec::new(),
        }
    }

    /// How Gate handles the subtask `request` gives the permanent agent
    /// `definition`, set up with `config`. A model the call names is
    /// applied to `config` here and pins the subtask, as do a council
    /// seat, a definition naming its own model, and a pinned main model.
    /// Under a `/compare` run, the run's arm decides instead of the
    /// user's selection.
    fn subtask_gate(
        &self,
        request: &SubagentDispatchRequest,
        definition: &AgentDefinition,
        config: &mut AgentLoopConfig,
    ) -> SubtaskGate {
        let arm = self.compare_routing(request.parent_agent_id);
        let base = lock(&self.base);
        let explicit = explicit_model(request);
        if let Some(model) = explicit {
            base.limits.configure(config, model);
        }
        let mut pinned =
            explicit.is_some() || request.council_index.is_some() || definition.model.is_some();
        let mut candidates = base.gate.candidates.clone();
        let mut explore = true;
        match arm {
            Some(ArmRouting::Pinned(model)) => {
                if !pinned {
                    base.limits.configure(config, &model);
                }
                pinned = true;
            }
            Some(ArmRouting::Gate {
                candidates: arm_candidates,
                ..
            }) => {
                candidates = Some(arm_candidates);
                explore = false;
            }
            None => {}
        }
        SubtaskGate {
            kind: definition.name.clone(),
            classifier: base.gate.classifier.clone(),
            candidates: if pinned { None } else { candidates },
            lead_model: self.gate.model_of(request.parent_agent_id),
            limits: base.limits,
            live: Arc::clone(&self.gate),
            explore,
            escalation: None,
        }
    }

    /// What Gate routes the work of an agent running the permanent agent
    /// `definition` among: `None` when the user pinned the main model or
    /// the definition names its own.
    pub(crate) fn gate_candidates(&self, definition: Option<&str>) -> Option<Vec<Candidate>> {
        let candidates = lock(&self.base).gate.candidates.clone()?;
        let pinned = definition.is_some_and(|name| {
            lock(&self.definitions)
                .get(name)
                .is_some_and(|d| d.model.is_some())
        });
        (!pinned).then_some(candidates)
    }

    /// The model that judges `ended`, the outcome of work Gate routed for
    /// an agent running the permanent agent `definition` (see
    /// [`gate_record::judge_for`]).
    pub(crate) fn judge_of(
        &self,
        definition: Option<&str>,
        ended: &OutcomePayload,
    ) -> Option<String> {
        let candidates = self.gate_candidates(definition)?;
        gate_record::judge_for(&self.gate, &candidates, ended)
    }

    /// The model that classifies prompts; empty when nothing does.
    pub(crate) fn classifier(&self) -> String {
        lock(&self.base).gate.classifier.clone()
    }

    /// For pointing an agent loop at a model other than the selected one.
    pub(crate) fn model_limits(&self) -> ModelLimits {
        lock(&self.base).limits
    }

    /// Tokens and cost per model of every model call `agent` and the
    /// agents under it made, classifier calls included.
    pub(crate) fn tokens_by_model(&self, agent: AgentId) -> Vec<ModelTokens> {
        let mut calls = Vec::new();
        self.each_entry(agent, |entry| {
            calls.extend(crate::record_loader::model_call(&entry.tx));
        });
        tally(calls)
    }

    /// The plan usage every model call of `agent` and the agents under it
    /// reported, with when each call was recorded.
    pub(crate) fn plan_readings(&self, agent: AgentId) -> Vec<PlanReading> {
        let mut readings = Vec::new();
        self.each_entry(agent, |entry| {
            let recorded_ms = entry.tx.ts_ms;
            readings.extend(
                crate::record_loader::plan_usage(&entry.tx)
                    .into_iter()
                    .map(|usage| PlanReading { recorded_ms, usage }),
            );
        });
        readings
    }

    /// The models the prompts of `agent` and the agents under it were
    /// routed to, first used first.
    pub(crate) fn routed_models(&self, agent: AgentId) -> Vec<String> {
        let mut models: Vec<String> = Vec::new();
        self.each_entry(agent, |entry| {
            if let Some(prompt) = entry.tx.prompt_payload() {
                let model = prompt.route.model;
                if !model.is_empty() && !models.contains(&model) {
                    models.push(model);
                }
            }
        });
        models
    }

    /// Call `f` with every entry in the records of `agent` and the agents
    /// under it, `agent`'s first.
    fn each_entry(&self, agent: AgentId, mut f: impl FnMut(&RecordEntry)) {
        let agents: Vec<AgentId> = std::iter::once(agent)
            .chain(lock(&self.registry).descendants(agent))
            .collect();
        let store = self.factory.store.as_ref();
        for agent in agents {
            let mut from = 1;
            loop {
                let entries = match store.scan_record(agent, from, SCAN_BATCH) {
                    Ok(entries) => entries,
                    Err(e) => {
                        warn!(%agent, error = %e, "Could not read an agent's record");
                        break;
                    }
                };
                entries.iter().for_each(&mut f);
                match entries.last() {
                    Some(last) if entries.len() == SCAN_BATCH => from = last.seq + 1,
                    _ => break,
                }
            }
        }
    }

    /// Route `run`'s subtask, run its turn to the end, run its check and
    /// record how it went, then hand its session back for the event loop
    /// to keep.
    async fn finish_task(&self, run: TaskRun) -> TaskDone {
        let TaskRun {
            mut session,
            mut config,
            tools,
            prompt,
            cancel,
            gate,
            mut summary,
            name,
            parent,
            check,
            mut trace,
        } = run;
        let child_id = session.agent_id();
        let ui = AgentUi::new(self.commands.clone(), child_id);
        let started = Instant::now();
        let label = gate.label(&session, &mut config, &prompt, &cancel).await;
        summary.model = Some(config.model.clone());
        trace.model.clone_from(&config.model);
        let recorder = self.traces.start(child_id, trace);
        self.register_root(child_id, &name, session.kernel.agent_workspace(), &config);
        self.processes.start(summary, Some(cancel.clone()));
        let files = self
            .factory
            .file_checkpoints
            .clone()
            .filter(|_| gate.candidates.is_some() && gate.escalation.is_none());
        if let Some(files) = &files {
            files.begin_attempt(child_id);
        }
        let agent_loop = AgentLoop::new(config);
        let mut end = run_task_turn(
            &mut session,
            &agent_loop,
            tools,
            &prompt,
            label,
            &ui,
            &cancel,
            self.long_run,
            recorder,
        )
        .await;
        let cancelled = cancel.is_cancelled();
        let check = match check {
            Some(command) if end.reply.is_ok() && !cancelled => {
                let run = task_check::run_check(&session, &command).await;
                ui.send(system_message(run.describe())).await;
                if let Some(signal) = run.signal() {
                    end.signal = signal;
                }
                Some(run)
            }
            _ => None,
        };
        let files_before = files.map(|files| files.take_attempt(child_id));
        let failure = task_check::Attempted {
            model: &agent_loop.config().model,
            reply: &end.reply,
            files: &end.files,
            check: check.as_ref(),
        }
        .failure()
        .filter(|_| !cancelled);
        self.processes.finish(
            &task_id(child_id),
            if end.reply.is_ok() {
                ProcessStatus::Exited
            } else {
                ProcessStatus::Failed
            },
        );
        let model = agent_loop.config().model.clone();
        let category = end.prompt.map(|(_, category)| category);
        let ended = end.prompt.map(|(prompt_tx, category)| OutcomePayload {
            prompt_tx,
            category,
            model: model.clone(),
            success: end.signal.success(),
            signal: end.signal,
            cost_usd: end.cost_usd,
            latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        });
        if let Some(ended) = &ended {
            gate_record::record(&session.kernel, child_id, ended).await;
        }
        // A first attempt that failed is reviewed before it is retried on
        // a stronger model: was the model at fault?
        let mut ended = ended;
        let mut judged = None;
        if let (Some(outcome), Some(failure), Some(candidates), None) = (
            ended.as_mut(),
            &failure,
            gate.candidates.as_deref(),
            &gate.escalation,
        ) {
            if let Some(judge) = gate_record::judge_for(&self.gate, candidates, outcome) {
                let workspace = session.kernel.agent_workspace();
                let diff = files_before
                    .as_ref()
                    .map(|files| files.diff(Some(&workspace)))
                    .unwrap_or_default();
                let failed = Failed {
                    task: &prompt,
                    diff: &diff,
                    reply: end.reply.as_deref().unwrap_or_default(),
                    failure,
                };
                let auth_token = agent_loop.config().auth_token.clone();
                if let Some((judged_as, found)) =
                    gate_record::judge_failed(&session.kernel, &judge, outcome, &failed, auth_token)
                        .await
                {
                    ui.send(system_message(found.describe())).await;
                    *outcome = judged_as;
                    judged = Some(found.verdict);
                }
            }
        }
        if let Some(redo) = self.end_attempt(parent, child_id, ended) {
            redo.record().await;
        }

        self.park(session);
        self.send_agent_list().await;
        TaskDone {
            child: child_id,
            name,
            model,
            reply: end.reply,
            category,
            files: end.files,
            check,
            files_before,
            cancelled,
            judged,
        }
    }
}

/// A spawned permanent agent's turn, ready to run.
struct TaskRun {
    session: AgentSession,
    /// Its agent loop's config, before Gate routes it
    config: AgentLoopConfig,
    tools: Vec<ToolDefinition>,
    prompt: String,
    cancel: CancellationToken,
    gate: SubtaskGate,
    /// Its Processes tab row
    summary: ProcessSummary,
    /// The instance's name
    name: String,
    /// The agent that dispatched it
    parent: AgentId,
    /// Command run after its turn, whose exit status is its verdict
    check: Option<String>,
    /// Its run trace, started once Gate has picked its model
    trace: RunMeta,
}

/// A `task` call being worked on.
struct Subtask {
    request: SubagentDispatchRequest,
    definition: AgentDefinition,
    first: TaskRun,
}

/// How one attempt at a subtask ended.
struct TaskDone {
    child: AgentId,
    /// The instance's name
    name: String,
    /// Model it ran on
    model: String,
    /// Its final reply, or why there is none
    reply: Result<String, String>,
    /// Its prompt's category, when the prompt was recorded
    category: Option<PromptCategory>,
    /// Files its file tools changed
    files: Vec<String>,
    /// How its check went, when it had one and got that far
    check: Option<task_check::CheckRun>,
    /// What the files it changed held before, when it may be retried
    files_before: Option<AttemptFiles>,
    cancelled: bool,
    /// What Gate's judge found, when the attempt failed and was reviewed
    judged: Option<Verdict>,
}

impl TaskDone {
    fn attempted(&self) -> task_check::Attempted<'_> {
        task_check::Attempted {
            model: &self.model,
            reply: &self.reply,
            files: &self.files,
            check: self.check.as_ref(),
        }
    }
}

/// How a subtask ended: its first attempt and, when that failed and
/// Gate retried it, the retry and why the first attempt failed.
struct TaskReport {
    first: TaskDone,
    retry: Option<(TaskDone, String)>,
}

impl TaskReport {
    /// The attempt whose result the caller gets.
    fn last(&self) -> &TaskDone {
        self.retry.as_ref().map_or(&self.first, |(retry, _)| retry)
    }

    /// The compact result the caller gets.
    fn text(&self) -> String {
        let retried = self
            .retry
            .as_ref()
            .map(|(_, why)| (self.first.model.as_str(), why.as_str()));
        task_check::compact(&self.last().attempted(), retried)
    }

    /// The reply, or why there is none, for a background task's notice.
    fn outcome(&self) -> Result<String, String> {
        match &self.last().reply {
            Ok(_) => Ok(self.text()),
            Err(reason) => Err(reason.clone()),
        }
    }

    /// The result of the `task` call.
    fn result(self) -> SubagentResult {
        let text = self.text();
        let last = self.retry.map_or(self.first, |(retry, _)| retry);
        let (final_message, exit) = match last.reply {
            Ok(_) => (text, SubagentExit::Completed),
            Err(reason) if text.is_empty() => (text, SubagentExit::Failed { reason }),
            Err(reason) => (
                String::new(),
                SubagentExit::Failed {
                    reason: format!("{reason}\n\n{text}"),
                },
            ),
        };
        SubagentResult {
            child_agent_id: Some(last.child),
            final_message,
            total_input_tokens: 0,
            total_output_tokens: 0,
            files_changed: last.files,
            exit,
        }
    }
}

/// A write subtask's turn at its workspace; the next one there starts
/// when it is dropped.
struct WriteSlot<'a> {
    hub: &'a AgentHub,
    /// The workspace's lock; `None` for a subtask of one that holds it,
    /// or when the wait was cancelled
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    /// Agents working in the slot, whose own write subtasks need not wait
    holders: Vec<AgentId>,
}

impl WriteSlot<'_> {
    fn hold(&mut self, agent: AgentId) {
        lock(&self.hub.writing).insert(agent);
        self.holders.push(agent);
    }

    /// Whether this slot took the workspace's lock itself.
    fn exclusive(&self) -> bool {
        self.guard.is_some()
    }
}

/// What `workspace` holds now, when it is in a git repository, so a
/// failed attempt's changes (shell commands' included) can be undone.
async fn snapshot_workspace(workspace: PathBuf) -> Option<changes::Snapshot> {
    let taken = tokio::task::spawn_blocking(move || changes::snapshot(&workspace)).await;
    match taken {
        Ok(Ok(snapshot)) => snapshot,
        Ok(Err(e)) => {
            warn!(error = %e, "Could not snapshot the workspace; only file tool changes will be undone");
            None
        }
        Err(e) => {
            warn!(error = %e, "Workspace snapshot task failed");
            None
        }
    }
}

/// Put the workspace back as `snapshot` recorded it.
async fn restore_workspace(snapshot: changes::Snapshot) {
    match tokio::task::spawn_blocking(move || snapshot.restore()).await {
        Ok(Ok(restored)) => {
            for path in &restored.blocked {
                warn!(path = %path.display(), "Could not undo a failed subtask's change: a folder with ignored files is in the way");
            }
            info!(
                written = restored.written.len(),
                removed = restored.removed.len(),
                "Undid a failed subtask's changes to the workspace"
            );
        }
        Ok(Err(e)) => {
            warn!(error = %e, "Could not undo a failed subtask's changes to the workspace");
        }
        Err(e) => warn!(error = %e, "Workspace restore task failed"),
    }
}

impl Drop for WriteSlot<'_> {
    fn drop(&mut self) {
        let mut writing = lock(&self.hub.writing);
        for agent in &self.holders {
            writing.remove(agent);
        }
    }
}

/// Whether subtasks of `definition` only read: it lists its tools and
/// none of them changes files. They run side by side; the others run one
/// at a time per workspace.
fn reads_only(definition: &AgentDefinition) -> bool {
    definition.tools.as_ref().is_some_and(|tools| {
        tools
            .iter()
            .all(|tool| aura_agent_gate::lead::keeps_tool(tool))
    })
}

/// What `parent` is told when its background agent `name` finishes.
fn task_notice(
    parent: AgentId,
    name: &str,
    todo: Option<&str>,
    outcome: Result<String, String>,
) -> Notice {
    let report = |ok| {
        todo.map(|todo| format!("\n\n{}", task_report_line(todo, ok)))
            .unwrap_or_default()
    };
    match outcome {
        Ok(reply) => Notice {
            to: parent,
            shown: format!("{name} finished its background task"),
            prompt: format!(
                "{name} finished the task you gave it in the background. Its reply:\n\n{reply}{}",
                report(true)
            ),
        },
        Err(reason) => Notice {
            to: parent,
            shown: format!("{name} did not finish its background task: {reason}"),
            prompt: format!(
                "{name} did not finish the task you gave it in the background: {reason}{}",
                report(false)
            ),
        },
    }
}

/// `task` tool schema listing the current permanent agents, or `None` when
/// there are none to spawn.
#[must_use]
pub fn task_tool_definition(definitions: &AgentDefinitions) -> Option<ToolDefinition> {
    if definitions.is_empty() {
        return None;
    }
    let names: Vec<&str> = definitions.all().iter().map(|d| d.name.as_str()).collect();
    let catalog: Vec<String> = definitions.all().iter().map(catalog_entry).collect();
    Some(ToolDefinition::new(
        TASK_TOOL_NAME,
        format!(
            "Spawn the permanent agent best suited to a task and wait for its final reply. \
             Pick it by matching the job against the descriptions below. \
             It works in the same workspace but with its own rules, tools and \
             context, so give it a complete, self-contained prompt. With \
             spawn_mode `detached` the call returns at once and the agent's \
             reply arrives later as a message, so you can keep working or \
             start several agents side by side. Agents that only read run side by \
             side; agents that edit files run one at a time. Give `check` a command \
             (such as a focused test) to run when the agent finishes: its exit status \
             says whether the task passed, and you get the agent's summary, the files \
             it changed and the check's result back.\n\
             Permanent agents:\n{}",
            catalog.join("\n")
        ),
        serde_json::json!({
            "type": "object",
            "properties": {
                "subagent_type": {
                    "type": "string",
                    "enum": names,
                    "description": "Name of the permanent agent to spawn."
                },
                "prompt": {
                    "type": "string",
                    "description": "Everything the agent needs to know to do the task."
                },
                "spawn_mode": {
                    "type": "string",
                    "enum": ["wait", "detached"],
                    "description": "`wait` (default) blocks until the agent replies; `detached` runs it in the background."
                },
                "check": {
                    "type": "string",
                    "description": "Command run in the workspace when the agent finishes, such as `cargo test -p parser`; exit status 0 means the task passed."
                },
                "todo_id": {
                    "type": "string",
                    "description": "Id of the todo this task does. The todo shows the agent and goes in progress now; when the agent reports back it is marked done, or pending again when the task failed."
                }
            },
            "required": ["subagent_type", "prompt"]
        }),
    ))
}

/// `- name: description (model: m; tools: a, b)`: one agent of the `task`
/// tool's catalog, with what it runs on when that is set.
fn catalog_entry(definition: &AgentDefinition) -> String {
    let mut facts = Vec::new();
    if let Some(model) = &definition.model {
        facts.push(format!("model: {model}"));
    }
    if let Some(tools) = definition.tools.as_ref().filter(|tools| !tools.is_empty()) {
        facts.push(format!("tools: {}", tools.join(", ")));
    }
    let mut entry = format!("- {}: {}", definition.name, definition.description);
    if !facts.is_empty() {
        entry.push_str(&format!(" ({})", facts.join("; ")));
    }
    entry
}

/// Processes tab row for the `task` spawn running as `child`, asked
/// `prompt`.
fn task_summary(
    request: &SubagentDispatchRequest,
    prompt: &str,
    name: &str,
    definition: &str,
    child: AgentId,
    model: &str,
) -> ProcessSummary {
    let owner = request.parent_agent_id;
    ProcessSummary {
        id: task_id(child),
        kind: ProcessKind::Task,
        label: format!("{name} ({definition})"),
        detail: prompt.to_string(),
        status: ProcessStatus::Running,
        owner_agent_id: owner.to_hex(),
        owner_name: String::new(),
        parent_chain: request
            .parent_chain
            .iter()
            .filter(|id| **id != owner)
            .map(AgentId::to_hex)
            .collect(),
        started_ms: now_ms(),
        tool_use_id: request.tool_call_id.clone(),
        model: Some(model.to_string()),
        jump_agent_id: child.to_hex(),
        killable: true,
        ..ProcessSummary::default()
    }
}

/// How a subtask's turn ended.
struct TaskEnd {
    /// The final reply, or why there is none
    reply: Result<String, String>,
    /// The recorded prompt and its category, when it was recorded
    prompt: Option<(Hash, PromptCategory)>,
    /// What the turn's end says about how it went
    signal: OutcomeSignal,
    /// List-price cost of the turn's model calls
    cost_usd: f64,
    /// Files the turn's file tools changed
    files: Vec<String>,
}

impl TaskEnd {
    const fn failed(reason: String) -> Self {
        Self {
            reply: Err(reason),
            prompt: None,
            signal: OutcomeSignal::Error,
            cost_usd: 0.0,
            files: Vec::new(),
        }
    }
}

/// Record `prompt` on `session` as `label` says and run one turn of it,
/// streaming it into `ui`. A subtask Gate routed was classified already;
/// a pinned one is classified beside its turn.
#[allow(clippy::too_many_arguments)]
async fn run_task_turn(
    session: &mut AgentSession,
    agent_loop: &AgentLoop,
    tools: Vec<ToolDefinition>,
    prompt: &str,
    label: SubtaskLabel,
    ui: &AgentUi,
    cancel: &CancellationToken,
    limits: LongRunSettings,
    recorder: Option<Arc<RunRecorder>>,
) -> TaskEnd {
    let turn_start = session.messages.len();
    let mut recorded = None;
    if let Some(category) = label.category {
        match label.record(session, category).await {
            Ok(prompt) => recorded = Some(prompt),
            Err(problem) => return TaskEnd::failed(format!("recording prompt: {problem}")),
        }
    }
    session.messages.push(Message::user(prompt));
    ui.send(UiCommand::TurnStarted {
        prompt: Some(prompt.to_string()),
    })
    .await;
    if let Some(line) = &label.shown {
        ui.send(system_message(line.clone())).await;
    }

    if let Some(rec) = &recorder {
        rec.begin_turn(prompt);
    }
    // Nothing sends to a spawn mid-task, so its steering stays empty.
    let steering = Steering::new(cancel);
    let shared: &AgentSession = session;
    let turn = run_agent_turn(
        shared,
        agent_loop,
        tools,
        ui,
        cancel,
        &steering,
        limits,
        recorder.clone(),
    );
    let (result, forwarded) = if recorded.is_some() {
        turn.await
    } else {
        let (late, turn) = tokio::join!(label.classify_and_record(shared, cancel), turn);
        match late {
            Ok(prompt) => recorded = Some(prompt),
            Err(problem) => warn!(%problem, "Failed to record the subtask's prompt"),
        }
        turn
    };
    steering.finish(&[]);
    if let Some(rec) = &recorder {
        super::trace::end_turn(rec, &result, cancel.is_cancelled());
    }
    let cancelled = cancel.is_cancelled();
    let signal = if cancelled || forwarded.credentials_rejected {
        OutcomeSignal::None
    } else {
        ended_signal(&result, turn_start)
    };
    let mut end = TaskEnd {
        reply: Ok(String::new()),
        prompt: recorded,
        signal,
        cost_usd: forwarded.usage.cost_usd,
        files: Vec::new(),
    };
    let result = match result {
        Ok(result) => result,
        Err(e) => {
            let reason = if cancelled {
                "cancelled by the user".to_string()
            } else {
                e.to_string()
            };
            ui.send(UiCommand::ShowWarning(reason.clone())).await;
            ui.send(UiCommand::Complete).await;
            end.reply = Err(reason);
            return end;
        }
    };
    let text = result.total_text.clone();
    let llm_error = result.llm_error.as_ref().map(ToString::to_string);
    end.files = result
        .file_changes
        .iter()
        .map(|change| change.path.clone())
        .collect();
    handle_agent_success(session, result, &forwarded, ui, &[]).await;
    end.reply = match llm_error {
        _ if cancel.is_cancelled() => Err("cancelled by the user".to_string()),
        Some(err) if text.trim().is_empty() => Err(err),
        _ => Ok(text),
    };
    end
}

/// Runs `task` calls by spawning permanent agents.
struct TerminalSubagentDispatcher {
    hub: Weak<AgentHub>,
}

#[async_trait]
impl SubagentDispatchHook for TerminalSubagentDispatcher {
    async fn dispatch(&self, request: SubagentDispatchRequest) -> Result<SubagentResult, String> {
        let Some(hub) = self.hub.upgrade() else {
            return Ok(SubagentResult::rejected(
                "task: the terminal is shutting down",
            ));
        };
        hub.spawn(request).await
    }
}

#[cfg(test)]
mod tests;
