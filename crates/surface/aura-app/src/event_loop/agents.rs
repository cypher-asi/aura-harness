//! Per-agent sessions, agent switching, and permanent agent management for
//! the terminal UI.
//!
//! Every registered agent gets its own kernel (and therefore its own
//! record) plus its own model conversation. Kernels are opened lazily the
//! first time an agent is selected and kept for the rest of the run, so
//! switching back resumes the in-memory conversation. Switching only
//! changes which agent the UI shows; agents keep working meanwhile.

use super::agent_events::AgentUi;
use super::subagents::lock;
use super::tasks::TaskBoard;
use super::LoopState;
use crate::agent_registry::{Opened, RegisteredAgent};
use crate::file_checkpoints::FileCheckpoints;
use crate::record_loader::{conversation_history, latest_plan_path, recent_record_summaries};
use crate::session_checkpoint::{
    is_resumable, now_ms, Checkpoints, SessionCheckpoint, SessionUsage,
};
use aura_agent::{
    CheckNotifier, CheckOutcome, CheckReport, CheckSource, KernelModelGateway, KernelToolGateway,
    PostEditCheck, PostEditCheckConfig,
};
use aura_agent_kernel::Kernel;
use aura_core_types::{AgentId, Identity};
use aura_exec_tools::agents::task::TASK_TOOL_NAME;
use aura_exec_tools::todo::{
    apply_todo_write, assign_task, report_task, task_reports, task_todo, TodoItem,
    TODO_WRITE_TOOL_NAME,
};
use aura_exec_tools::RunScratch;
use aura_model::{ContentBlock, Message, Role};
use aura_store_record::ReadStore;
use aura_ui_terminal::UiCommand;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Live state for one agent.
pub struct AgentSession {
    pub identity: Identity,
    pub kernel: Arc<Kernel>,
    pub model_gateway: KernelModelGateway,
    pub tool_gateway: KernelToolGateway,
    /// Model conversation for this agent's current session.
    pub messages: Vec<Message>,
    /// Permanent agent this is an instance of; its turns follow its rules.
    pub definition: Option<String>,
    /// The agent's `todo_write` list, carried from turn to turn.
    pub todos: Vec<TodoItem>,
    /// When the current session started; names its checkpoint
    pub started_ms: u64,
    /// Model calls and tokens of the current session
    pub usage: SessionUsage,
    /// Where the session is checkpointed after each turn, if anywhere
    pub checkpoints: Option<Checkpoints>,
    /// Where files are checkpointed before each turn changes them, if anywhere
    pub file_checkpoints: Option<FileCheckpoints>,
    /// Runs the project's check after steps that edit files
    pub post_edit_check: Option<PostEditCheck>,
    /// Workspace-relative path of the plan this agent last wrote, which
    /// `/build` implements.
    pub active_plan: Option<String>,
    /// Whether the agent is building `active_plan` (after `/build`), so its
    /// todo list is mirrored into the plan file.
    pub building_plan: bool,
    /// Whether its current turn runs as Gate's lead with delegation
    /// enforced (see [`aura_gate::lead`]); set where the turn's tools are put
    /// together.
    pub lead: bool,
    /// Where its work is kept as tasks
    pub tasks: Arc<TaskBoard>,
}

impl AgentSession {
    #[must_use]
    pub const fn agent_id(&self) -> AgentId {
        self.identity.agent_id
    }

    /// Start a new, empty session.
    pub fn reset(&mut self) {
        self.messages.clear();
        self.todos.clear();
        self.usage = SessionUsage::default();
        self.started_ms = now_ms();
        self.building_plan = false;
    }

    /// Pick the plan build back up after the session was reopened or
    /// restored: the in-memory flag is gone, but a todo list that is still
    /// the active plan's means `/build` is under way.
    pub fn resume_plan_build(&mut self) {
        self.building_plan = self.active_plan.as_deref().is_some_and(|path| {
            super::plan_mode::is_plan_todo_list(&self.kernel.agent_workspace(), path, &self.todos)
        });
    }

    /// Continue the session saved in `checkpoint`.
    pub fn restore(&mut self, checkpoint: SessionCheckpoint) {
        self.messages = checkpoint.messages;
        if !is_resumable(&self.messages) {
            self.messages.pop();
        }
        self.todos = checkpoint.todos;
        self.usage = checkpoint.usage;
        self.started_ms = checkpoint.started_ms;
        self.resume_plan_build();
    }

    /// Checkpoint the session as of the head of its record. A failure is
    /// logged; the record still has the conversation.
    pub async fn save_checkpoint(&self) {
        let Some(checkpoints) = &self.checkpoints else {
            return;
        };
        let agent_id = self.agent_id();
        let record_seq = match self.kernel.store().get_head_seq(agent_id) {
            Ok(seq) => seq,
            Err(e) => {
                warn!(%agent_id, error = %e, "Skipping checkpoint: record head unknown");
                return;
            }
        };
        let checkpoint = SessionCheckpoint::new(
            self.started_ms,
            record_seq,
            self.messages.clone(),
            self.todos.clone(),
            self.usage.clone(),
        );
        if let Err(e) = checkpoints.save(agent_id, &checkpoint).await {
            warn!(%agent_id, error = %e, "Failed to save session checkpoint");
        }
    }
}

/// The todo list left by the `todo_write` calls in `messages`, with the
/// todos `task` calls gave to agents assigned and, once the agents
/// reported back (in the call's result, or a detached task's notice),
/// closed: the list the agent loop kept as the turns ran.
pub(crate) fn todos_from_history(messages: &[Message]) -> Vec<TodoItem> {
    let mut todos = Vec::new();
    let mut tasks: HashMap<&str, &serde_json::Value> = HashMap::new();
    for message in messages {
        for block in &message.content {
            let next = match block {
                ContentBlock::ToolUse { name, input, .. } if name == TODO_WRITE_TOOL_NAME => {
                    apply_todo_write(&todos, input).ok()
                }
                ContentBlock::ToolUse { id, name, input } if name == TASK_TOOL_NAME => {
                    tasks.insert(id, input);
                    assign_task(&todos, input)
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    is_error,
                    ..
                } => tasks
                    .get(tool_use_id.as_str())
                    .and_then(|input| task_todo(input))
                    .filter(|(_, reports)| *reports)
                    .and_then(|(id, _)| report_task(&todos, id, !is_error)),
                ContentBlock::Text { text } if message.role == Role::User => {
                    let mut next = None;
                    for (id, ok) in task_reports(text) {
                        let current = next.as_ref().unwrap_or(&todos);
                        if let Some(reported) = report_task(current, &id, ok) {
                            next = Some(reported);
                        }
                    }
                    next
                }
                _ => None,
            };
            if let Some(next) = next {
                todos = next;
            }
        }
    }
    todos
}

/// Constructs the kernel for one agent, given the agents that spawned it
/// (nearest first) and its project's folder (`None` uses the launch
/// directory).
pub type KernelBuilder =
    Box<dyn Fn(AgentId, Vec<AgentId>, Option<PathBuf>) -> anyhow::Result<Kernel> + Send + Sync>;

/// Opens per-agent sessions and reads agent records for the UI.
pub struct KernelFactory {
    pub(crate) store: Arc<dyn ReadStore>,
    build_kernel: KernelBuilder,
    pub(super) checkpoints: Option<Checkpoints>,
    pub(super) file_checkpoints: Option<FileCheckpoints>,
    post_edit_check: Option<(PostEditCheckConfig, mpsc::Sender<UiCommand>)>,
    /// Every agent's current task; the hub gives it a way to show them
    pub(crate) tasks: Arc<TaskBoard>,
    /// Prompt-cache scope of each agent whose kernel is scoped: a
    /// `/compare` run's agents, and the agents opened under them
    cache_salts: Mutex<HashMap<AgentId, String>>,
    /// Scratch folder of each agent whose commands get their own: a
    /// `/compare` run's agents, and the agents opened under them
    run_scratches: Mutex<HashMap<AgentId, RunScratch>>,
}

impl KernelFactory {
    #[must_use]
    pub fn new(store: Arc<dyn ReadStore>, build_kernel: KernelBuilder) -> Self {
        Self {
            store,
            build_kernel,
            checkpoints: None,
            file_checkpoints: None,
            post_edit_check: None,
            tasks: Arc::default(),
            cache_salts: Mutex::default(),
            run_scratches: Mutex::default(),
        }
    }

    /// Scope the prompt cache of `agent`'s kernels, and of the kernels of
    /// the agents opened under it, to `salt` from the next
    /// [`Self::open`] on.
    pub(crate) fn scope_cache(&self, agent: AgentId, salt: String) {
        lock(&self.cache_salts).insert(agent, salt);
    }

    /// The prompt-cache scope of `agent`'s kernels, if they have one.
    pub(crate) fn cache_salt_of(&self, agent: AgentId) -> Option<String> {
        lock(&self.cache_salts).get(&agent).cloned()
    }

    /// Run `agent`'s commands, and those of the agents opened under it,
    /// with `scratch` from the next [`Self::open`] on.
    pub(crate) fn give_scratch(&self, agent: AgentId, scratch: RunScratch) {
        lock(&self.run_scratches).insert(agent, scratch);
    }

    /// The scratch folder of `agent`'s commands, if they have one.
    pub(crate) fn run_scratch_of(&self, agent: AgentId) -> Option<RunScratch> {
        lock(&self.run_scratches).get(&agent).cloned()
    }
}

/// The entry of `agent` in `by_agent`, else of the nearest agent in
/// `parent_chain` with one, remembered as `agent`'s so a later open
/// without the chain (the session reopened from its record) keeps it.
fn inherited<T: Clone>(
    by_agent: &Mutex<HashMap<AgentId, T>>,
    agent: AgentId,
    parent_chain: &[AgentId],
) -> Option<T> {
    let mut by_agent = lock(by_agent);
    if let Some(value) = by_agent.get(&agent) {
        return Some(value.clone());
    }
    let value = parent_chain
        .iter()
        .find_map(|ancestor| by_agent.get(ancestor))
        .cloned()?;
    by_agent.insert(agent, value.clone());
    Some(value)
}

impl KernelFactory {
    /// Give sessions `files` to checkpoint each turn's file changes in. The
    /// kernels' executors must report changes to the same checkpoints.
    #[must_use]
    pub fn with_file_checkpoints(mut self, files: FileCheckpoints) -> Self {
        self.file_checkpoints = Some(files);
        self
    }

    /// Give every session a post-edit check with `config`, reporting its
    /// results to the agent's chat through `commands`.
    #[must_use]
    pub fn with_post_edit_check(
        mut self,
        config: PostEditCheckConfig,
        commands: mpsc::Sender<UiCommand>,
    ) -> Self {
        self.post_edit_check = Some((config, commands));
        self
    }

    /// Checkpoint sessions in `checkpoints` and resume them from there.
    #[must_use]
    pub fn with_checkpoints(mut self, checkpoints: Checkpoints) -> Self {
        self.checkpoints = Some(checkpoints);
        self
    }

    /// The agent's current checkpoint with what the record holds beyond
    /// it, when the checkpoint still applies.
    fn restore(&self, agent_id: AgentId) -> Option<SessionCheckpoint> {
        let mut checkpoint = self.checkpoints.as_ref()?.load_current(agent_id)?;
        let newer = crate::record_loader::model_history_since(
            self.store.as_ref(),
            agent_id,
            checkpoint.record_seq,
        )
        .unwrap_or_else(|e| {
            warn!(error = %e, "Failed to read the record past the checkpoint");
            None
        })?;
        if !is_resumable(&checkpoint.messages) {
            checkpoint.messages.pop();
        }
        checkpoint.messages.extend(newer);
        Some(checkpoint)
    }

    /// Open a session (kernel + gateways) for `agent`, working in `folder`
    /// (its project's folder, `None` for the launch directory). The model
    /// picks up the conversation the record holds for the agent.
    ///
    /// # Errors
    /// Returns an error if the kernel cannot be constructed (e.g. the
    /// store read or record replay fails).
    pub fn open(
        &self,
        agent: &RegisteredAgent,
        parent_chain: Vec<AgentId>,
        folder: Option<PathBuf>,
    ) -> anyhow::Result<AgentSession> {
        let identity = agent.identity();
        let salt = inherited(&self.cache_salts, identity.agent_id, &parent_chain);
        let scratch = inherited(&self.run_scratches, identity.agent_id, &parent_chain);
        let mut kernel = (self.build_kernel)(identity.agent_id, parent_chain, folder)?;
        if let Some(salt) = salt {
            kernel = kernel.with_cache_salt(salt);
        }
        if let Some(scratch) = scratch {
            kernel = kernel.with_run_scratch(scratch);
        }
        let kernel = Arc::new(kernel);
        let post_edit_check = self.post_edit_check.as_ref().map(|(config, commands)| {
            let check = PostEditCheck::new(kernel.agent_workspace(), config.clone())
                .with_notifier(check_notifier(commands.clone(), identity.agent_id));
            // The check builds where the run's own commands do, so the
            // arms of a comparison don't share its output either.
            match kernel.run_scratch() {
                Some(scratch) => check.with_run_scratch(scratch.clone()),
                None => check,
            }
        });
        let active_plan = latest_plan_path(self.store.as_ref(), identity.agent_id);
        let mut session = AgentSession {
            identity,
            model_gateway: KernelModelGateway::new(kernel.clone()),
            tool_gateway: KernelToolGateway::new(kernel.clone()),
            kernel,
            messages: Vec::new(),
            definition: agent.definition.clone(),
            todos: Vec::new(),
            started_ms: now_ms(),
            usage: SessionUsage::default(),
            checkpoints: self.checkpoints.clone(),
            file_checkpoints: self.file_checkpoints.clone(),
            post_edit_check,
            active_plan,
            building_plan: false,
            lead: false,
            tasks: Arc::clone(&self.tasks),
        };
        if let Some(checkpoint) = self.restore(session.agent_id()) {
            info!(agent = %session.agent_id(), "Resuming session from checkpoint");
            session.restore(checkpoint);
        } else {
            let messages =
                crate::record_loader::model_history(self.store.as_ref(), session.agent_id())
                    .unwrap_or_else(|e| {
                        warn!(error = %e, "Failed to rebuild model history from record");
                        Vec::new()
                    });
            session.todos = todos_from_history(&messages);
            session.messages = messages;
            session.resume_plan_build();
        }
        Ok(session)
    }
}

/// Shows post-edit check results in the agent's chat. The model gets the
/// failure output itself; the chat only says what happened.
fn check_notifier(commands: mpsc::Sender<UiCommand>, agent_id: AgentId) -> CheckNotifier {
    let agent_id = agent_id.to_hex();
    let announced = AtomicBool::new(false);
    Arc::new(move |report: &CheckReport| {
        let first = !announced.swap(true, Ordering::Relaxed);
        if let Some(command) = check_notice(report, first) {
            let _ = commands.try_send(UiCommand::ForAgent {
                agent_id: agent_id.clone(),
                command: Box::new(command),
            });
        }
    })
}

/// What the chat shows for a check run; `first` is the session's first run,
/// which names where the command comes from.
pub(super) fn check_notice(report: &CheckReport, first: bool) -> Option<UiCommand> {
    let command = &report.command.command;
    let origin = match report.command.source {
        CheckSource::Env => "AURA_CHECK_COMMAND",
        CheckSource::AgentsMd => "AGENTS.md",
    };
    match &report.outcome {
        CheckOutcome::Passed { recovered: true } => Some(UiCommand::ShowSuccess(format!(
            "Check `{command}` passes again"
        ))),
        CheckOutcome::Passed { recovered: false } if first => Some(UiCommand::ShowSuccess(
            format!("Check `{command}` (from {origin}) passed; it runs after each step that edits files"),
        )),
        CheckOutcome::Passed { .. } | CheckOutcome::StillFailing => None,
        CheckOutcome::Failed { timed_out: true, .. } => Some(UiCommand::ShowWarning(format!(
            "Check `{command}` (from {origin}) timed out; the agent was told"
        ))),
        CheckOutcome::Failed { exit_code, .. } => {
            let code = exit_code.map_or_else(String::new, |c| format!(" with exit code {c}"));
            Some(UiCommand::ShowWarning(format!(
                "Check `{command}` (from {origin}) failed{code}; the agent was given the output"
            )))
        }
        CheckOutcome::CouldNotRun(problem) => Some(UiCommand::ShowWarning(format!(
            "Check `{command}` (from {origin}) could not run: {problem}"
        ))),
    }
}

pub(super) async fn handle_select_agent(state: &mut LoopState<'_>, agent_hex: &str) {
    match AgentId::from_hex(agent_hex) {
        Ok(agent_id) => activate_agent(state, agent_id, Chat::Reload).await,
        Err(e) => {
            warn!(agent = agent_hex, error = %e, "Invalid agent id from UI");
            show_error(state, format!("Invalid agent id: {e}")).await;
        }
    }
}

/// Make the agent with hex id `agent_hex` the active one for a view that
/// already shows its chat.
pub(super) async fn handle_focus_agent(state: &mut LoopState<'_>, agent_hex: &str) {
    match AgentId::from_hex(agent_hex) {
        Ok(agent_id) => activate_agent(state, agent_id, Chat::Keep).await,
        Err(e) => warn!(agent = agent_hex, error = %e, "Invalid agent id from UI"),
    }
}

/// What the UI needs of an agent's chat when the agent becomes active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Chat {
    /// Rebuild it from the record, unless the agent is at work and the
    /// chat is live.
    Reload,
    /// The view asking already has it; rebuilding would lose its scroll
    /// position and what it has expanded.
    Keep,
}

/// Open `folder` as a project: a new one, the one it already is, or the
/// one it was before it was removed. Switches to its agent.
pub(super) async fn handle_create_project(state: &mut LoopState<'_>, folder: &Path) {
    let opened = {
        let mut registry = lock(&state.hub.registry);
        registry.open_project(folder).map(|(agent, opened)| {
            let project = registry.project_of(agent.agent_id()).cloned();
            (agent, opened, project)
        })
    };
    let (agent, opened, project) = match opened {
        Ok((agent, opened, Some(project))) => (agent, opened, project),
        Ok((_, _, None)) => {
            show_error(state, "Could not open the project".to_string()).await;
            return;
        }
        Err(e) => {
            show_error(state, format!("Could not open project: {e}")).await;
            return;
        }
    };
    let name = project.name();
    let path = project.folder.display().to_string();
    info!(zns_id = %agent.zns_id, project = %name, folder = %path, ?opened, "Opened project");
    activate_agent(state, agent.agent_id(), Chat::Reload).await;
    let notice = match opened {
        Opened::Created => UiCommand::ShowSuccess(format!("Created project {name} in {path}")),
        Opened::Existing => UiCommand::ShowWarning(format!("{path} is already project {name}")),
        Opened::Revived => UiCommand::ShowSuccess(format!("Reopened project {name}")),
    };
    let _ = state.commands.send(notice).await;
}

/// Save the order projects were dragged into.
pub(super) async fn handle_reorder_projects(state: &mut LoopState<'_>, order: &[String]) {
    let saved = lock(&state.hub.registry).reorder_projects(order);
    if let Err(e) = saved {
        show_error(state, format!("Could not save the order of projects: {e}")).await;
    }
    state.hub.send_agent_list().await;
}

/// Save the order a project's agents were dragged into.
pub(super) async fn handle_reorder_agents(
    state: &mut LoopState<'_>,
    project_id: &str,
    order: &[String],
) {
    let saved = lock(&state.hub.registry).reorder_agents(project_id, order);
    if let Err(e) = saved {
        show_error(state, format!("Could not save the order of agents: {e}")).await;
    }
    state.hub.send_agent_list().await;
}

pub(super) async fn handle_create_agent(state: &mut LoopState<'_>, name: Option<String>) {
    let created = lock(&state.hub.registry).create(name);
    match created {
        Ok(agent) => {
            info!(zns_id = %agent.zns_id, name = %agent.name, "Created terminal agent");
            activate_agent(state, agent.agent_id(), Chat::Reload).await;
            show_success(state, format!("Created agent {}", agent.name)).await;
        }
        Err(e) => show_error(state, format!("Could not create agent: {e}")).await,
    }
}

/// Add a new agent to project `project_id`, and switch to it.
pub(super) async fn handle_add_agent_to_project(state: &mut LoopState<'_>, project_id: &str) {
    let created = lock(&state.hub.registry).create_in_project(project_id);
    match created {
        Ok(agent) => {
            info!(zns_id = %agent.zns_id, name = %agent.name, "Added agent to project");
            activate_agent(state, agent.agent_id(), Chat::Reload).await;
            show_success(state, format!("Created agent {}", agent.name)).await;
        }
        Err(e) => show_error(state, format!("Could not create agent: {e}")).await,
    }
}

/// Parse a hex agent id from the UI, reporting a bad one.
async fn parse_agent(state: &LoopState<'_>, agent_hex: &str) -> Option<AgentId> {
    match AgentId::from_hex(agent_hex) {
        Ok(agent_id) => Some(agent_id),
        Err(e) => {
            warn!(agent = agent_hex, error = %e, "Invalid agent id from UI");
            show_error(state, format!("Invalid agent id: {e}")).await;
            None
        }
    }
}

/// Name of the first agent in `agents` with work in flight.
fn busy_agent(state: &LoopState<'_>, agents: &[AgentId]) -> Option<String> {
    let busy = agents.iter().find(|&&id| state.hub.is_running(id))?;
    Some(
        lock(&state.hub.registry)
            .find(*busy)
            .map_or_else(|| "an agent".to_string(), |a| a.display_name().into_owned()),
    )
}

/// Stop the work of every agent in `agents` that still has work in
/// flight. Returns the display name of each one stopped, in order.
///
/// A turn waiting on an approval is denied first so it sees the
/// cancellation rather than hanging until its prompt times out.
async fn stop_busy(state: &mut LoopState<'_>, agents: &[AgentId]) -> Vec<String> {
    let mut stopped = Vec::new();
    for &id in agents {
        if !state.hub.is_running(id) {
            continue;
        }
        state.approvals.cancel_agent(id).await;
        if state.hub.cancel(id) {
            let name = lock(&state.hub.registry)
                .find(id)
                .map_or_else(|| "an agent".to_string(), |a| a.display_name().into_owned());
            stopped.push(name);
        }
    }
    stopped
}

/// After agents left the list, show the one the registry opens now if the
/// active agent was among them; otherwise just refresh the list.
async fn after_list_shrank(state: &mut LoopState<'_>, gone: &[AgentId]) {
    // The agents leave the list at once; opening the next one can take a
    // while when it has a long record.
    state.hub.send_agent_list().await;
    if gone.contains(&state.active) {
        let next = lock(&state.hub.registry).active().agent_id();
        activate_agent(state, next, Chat::Reload).await;
    }
}

/// Move an agent, and the agents under it, to the Archive.
pub(super) async fn handle_archive_agent(state: &mut LoopState<'_>, agent_hex: &str) {
    let Some(agent_id) = parse_agent(state, agent_hex).await else {
        return;
    };
    let (subtree, name) = {
        let registry = lock(&state.hub.registry);
        (
            registry.subtree(agent_id),
            registry
                .find(agent_id)
                .map(|a| a.display_name().into_owned()),
        )
    };
    let Some(name) = name else {
        show_error(state, "Unknown agent".to_string()).await;
        return;
    };
    // Archiving is reversible, so it does not wait for work in flight to
    // finish on its own: it stops it and carries on.
    let stopped = stop_busy(state, &subtree).await;
    super::cloud::on_archive(state, &subtree);
    let archived = lock(&state.hub.registry).archive(agent_id);
    match archived {
        Ok(()) => {
            info!(%agent_id, %name, stopped = stopped.len(), "Archived agent");
            after_list_shrank(state, &subtree).await;
            let message = if stopped.is_empty() {
                format!("Archived {name}")
            } else {
                format!("Archived {name}; stopped {}", stopped.join(", "))
            };
            show_success(state, message).await;
        }
        Err(e) => show_error(state, format!("Could not archive {name}: {e}")).await,
    }
}

/// Bring an agent back from the Archive and switch to it.
pub(super) async fn handle_restore_agent(state: &mut LoopState<'_>, agent_hex: &str) {
    let Some(agent_id) = parse_agent(state, agent_hex).await else {
        return;
    };
    let restored = lock(&state.hub.registry).restore(agent_id);
    match restored {
        Ok(()) => {
            let name = lock(&state.hub.registry)
                .find(agent_id)
                .map(|a| a.display_name().into_owned())
                .unwrap_or_default();
            info!(%agent_id, %name, "Restored agent");
            activate_agent(state, agent_id, Chat::Reload).await;
            show_success(state, format!("Restored {name}")).await;
        }
        Err(e) => show_error(state, format!("Could not restore agent: {e}")).await,
    }
}

/// Remove project `project_id` from the list. Its records and folder stay
/// on disk.
pub(super) async fn handle_remove_project(state: &mut LoopState<'_>, project_id: &str) {
    let found = {
        let registry = lock(&state.hub.registry);
        registry.project(project_id).map(|project| {
            let agents: Vec<AgentId> = registry
                .agents()
                .iter()
                .filter(|a| a.project_id.as_deref() == Some(project_id))
                .map(RegisteredAgent::agent_id)
                .collect();
            (agents, project.name())
        })
    };
    let Some((agents, project)) = found else {
        show_error(state, "Unknown project".to_string()).await;
        return;
    };
    if let Some(busy) = busy_agent(state, &agents) {
        show_error(
            state,
            format!("Cannot remove {project}: {busy} is still working"),
        )
        .await;
        return;
    }
    let removed = lock(&state.hub.registry).remove_project(project_id);
    match removed {
        Ok(removed) => {
            for id in &removed {
                state.idle.remove(id);
            }
            info!(%project, agents = removed.len(), "Removed project");
            after_list_shrank(state, &removed).await;
            show_success(state, format!("Removed project {project}")).await;
        }
        Err(e) => show_error(state, format!("Could not remove {project}: {e}")).await,
    }
}

/// Start an instance of a permanent agent and switch to it.
pub(super) async fn handle_instantiate_definition(state: &mut LoopState<'_>, name: &str) {
    if let Some(agent) = start_instance(state, name, Placement::Alone).await {
        show_success(state, format!("Started {}", agent.name)).await;
    }
}

/// Where a new permanent-agent instance goes in the agents list.
enum Placement {
    /// The launch directory's project.
    Alone,
    /// At the top of the project with this id.
    InProject(String),
}

/// Start an instance of a permanent agent inside the project that contains
/// the active agent, and switch to it.
pub(super) async fn handle_add_definition_to_project(state: &mut LoopState<'_>, name: &str) {
    let project = lock(&state.hub.registry).project_of(state.active).cloned();
    let Some(project) = project else {
        show_error(state, "Unknown agent".to_string()).await;
        return;
    };
    if let Some(agent) = start_instance(state, name, Placement::InProject(project.id.clone())).await
    {
        show_success(state, format!("Added {} to {}", agent.name, project.name())).await;
    }
}

/// Register an instance of the permanent agent `name` at `placement` and
/// switch to it. Reports failures itself and returns `None`.
async fn start_instance(
    state: &mut LoopState<'_>,
    name: &str,
    placement: Placement,
) -> Option<RegisteredAgent> {
    let definition = lock(&state.hub.definitions)
        .get(name)
        .map(|d| d.name.clone());
    let Some(definition) = definition else {
        show_error(state, format!("No permanent agent named {name}")).await;
        return None;
    };
    let created = {
        let mut registry = lock(&state.hub.registry);
        match placement {
            Placement::Alone => registry.create_instance(&definition, None),
            Placement::InProject(project) => {
                registry.add_instance_to_project(&definition, &project)
            }
        }
    };
    match created {
        Ok(agent) => {
            info!(name = %agent.name, %definition, "Started permanent agent instance");
            activate_agent(state, agent.agent_id(), Chat::Reload).await;
            Some(agent)
        }
        Err(e) => {
            show_error(state, format!("Could not start {definition}: {e}")).await;
            None
        }
    }
}

/// Open a permanent agent's file in the editor, creating a new one when
/// `name` is `None`.
pub(super) async fn handle_edit_definition(state: &LoopState<'_>, name: Option<String>) {
    let path = {
        let mut definitions = lock(&state.hub.definitions);
        match name {
            Some(name) => definitions
                .get(&name)
                .map(|d| d.path.clone())
                .ok_or_else(|| format!("No permanent agent named {name}")),
            None => definitions
                .create(None)
                .map_err(|e| format!("Could not create a permanent agent: {e:#}")),
        }
    };
    match path {
        Ok(path) => {
            state.hub.send_definitions().await;
            let _ = state.commands.send(UiCommand::OpenEditor(path)).await;
        }
        Err(problem) => show_error(state, problem).await,
    }
}

pub(super) async fn handle_delete_definition(state: &LoopState<'_>, name: &str) {
    let deleted = lock(&state.hub.definitions).delete(name);
    match deleted {
        Ok(definition) => {
            info!(name = %definition.name, "Deleted permanent agent");
            state.hub.send_definitions().await;
            show_success(
                state,
                format!("Deleted permanent agent {}", definition.name),
            )
            .await;
        }
        Err(e) => show_error(state, format!("{e:#}")).await,
    }
}

/// Reload the permanent agents after the editor closed on `path`.
pub(super) async fn handle_definition_edited(state: &LoopState<'_>, path: &Path) {
    let problems = state.hub.reload_definitions();
    state.hub.send_definitions().await;
    let file = path.display().to_string();
    if let Some(problem) = problems.iter().find(|p| p.starts_with(&file)) {
        let _ = state
            .commands
            .send(UiCommand::ShowWarning(format!("Not loaded: {problem}")))
            .await;
        return;
    }
    let name = lock(&state.hub.definitions)
        .all()
        .iter()
        .find(|d| d.path == path)
        .map(|d| d.name.clone());
    if let Some(name) = name {
        show_success(state, format!("Saved permanent agent {name}")).await;
    }
}

pub(super) async fn handle_refresh_agents(state: &LoopState<'_>) {
    state.hub.send_agent_list().await;
    state.hub.send_definitions().await;
}

/// Open a session for `agent_id` from its record.
pub(super) fn open_session(
    state: &LoopState<'_>,
    agent_id: AgentId,
) -> Result<AgentSession, String> {
    let (registered, folder) = {
        let registry = lock(&state.hub.registry);
        (
            registry.find(agent_id).cloned(),
            registry.project_folder(agent_id),
        )
    };
    let Some(registered) = registered else {
        return Err("Unknown agent".to_string());
    };
    state
        .hub
        .factory
        .open(&registered, Vec::new(), folder)
        .map_err(|e| {
            warn!(error = %e, "Failed to open agent kernel");
            format!("Could not open agent {}: {e}", registered.name)
        })
}

/// Show `agent_id`. Agents keep working while another one is shown.
///
/// A UI switches on screen before it asks, so every select gets an
/// answer it can end its wait on: the agent's chat, [`UiCommand::ChatReady`]
/// for a live one, or [`UiCommand::SwitchFailed`].
async fn activate_agent(state: &mut LoopState<'_>, agent_id: AgentId, chat: Chat) {
    state.park_finished_spawns().await;
    if agent_id == state.active {
        if chat == Chat::Reload {
            // A UI that showed another agent in the meantime asks for this
            // one's chat again.
            show_active_agent(state, chat).await;
        } else {
            // Confirm it all the same: a UI that switched on screen ahead
            // waits for this answer before taking the session's next one.
            let _ = state
                .commands
                .send(UiCommand::SetActiveAgent(agent_id.to_hex()))
                .await;
            state.hub.send_agent_list().await;
        }
        return;
    }
    if !state.idle.contains_key(&agent_id) && !state.hub.is_running(agent_id) {
        match open_session(state, agent_id) {
            Ok(session) => {
                state.idle.insert(agent_id, session);
            }
            Err(problem) => {
                show_error(state, problem).await;
                let _ = state
                    .commands
                    .send(UiCommand::SwitchFailed(agent_id.to_hex()))
                    .await;
                return;
            }
        }
    }
    state.active = agent_id;

    if let Err(e) = lock(&state.hub.registry).set_active(agent_id) {
        warn!(error = %e, "Failed to persist active agent");
    }

    show_active_agent(state, chat).await;
}

/// Switch the UI to the active agent and fill the Record panel and, with
/// [`Chat::Reload`], the chat with its history. A busy agent's chat is
/// already live in the UI.
pub(super) async fn show_active_agent(state: &LoopState<'_>, chat: Chat) {
    let store = state.hub.factory.store.as_ref();
    let agent_id = state.active;

    state.hub.record_panel.set(agent_id);
    let _ = state
        .commands
        .send(UiCommand::SetActiveAgent(agent_id.to_hex()))
        .await;
    // The lists go first: a new or restored agent shows up in them, and an
    // archived one leaves them, before its history has been read.
    state.hub.send_agent_list().await;
    // The chat next: it is what the UI waits on, blank, while the rest
    // (records, traces) can fill in after it.
    if chat == Chat::Reload {
        if state.hub.is_running(agent_id) {
            // Its chat is live in the UI already; say so, so a UI that
            // switched to it ahead of this answer stops waiting.
            AgentUi::new(state.commands.clone(), agent_id)
                .send(UiCommand::ChatReady)
                .await;
        } else {
            let history = conversation_history(store, agent_id).unwrap_or_else(|e| {
                warn!(error = %e, "Failed to rebuild conversation from record");
                Vec::new()
            });
            AgentUi::new(state.commands.clone(), agent_id)
                .send(UiCommand::LoadConversation(history))
                .await;
            super::hud::send_session_of(state, agent_id).await;
        }
    }
    // One replace-all command: clears the previous agent's records even
    // when this agent's history can't be read.
    let (records, problem) = match recent_record_summaries(store, agent_id) {
        Ok(summaries) => (summaries, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    let has_records = store.get_head_seq(agent_id).map_or(true, |head| head > 0);
    let _ = state.commands.send(UiCommand::SetRecords(records)).await;
    if let Some(e) = problem {
        let _ = state
            .commands
            .send(UiCommand::ShowWarning(format!(
                "Could not load record history: {e}"
            )))
            .await;
    }
    // An agent with nothing recorded has never run, so it has no stored
    // run to look for, and looking finds none only after reading every
    // run's metadata.
    if has_records || state.hub.is_running(agent_id) {
        state.hub.traces.send_history(agent_id).await;
    }

    super::modes::send_mode(state, agent_id).await;
    super::policies::send_policies_of(state, agent_id).await;
    super::hud::send_instructions_of(state, agent_id).await;
    super::plan_mode::send_plans_of(state, agent_id).await;
    if state.hub.is_running(agent_id) {
        // Its turn shows its plan again when it next changes.
        let _ = state.commands.send(UiCommand::SetPlan(None)).await;
    } else {
        super::plan_mode::send_plan_of(state, agent_id).await;
    }

    send_sandbox_root(state).await;
}

/// Point the Sandbox panel at the active agent's workspace, creating it
/// so the panel has something to show before the first tool call.
pub(super) async fn send_sandbox_root(state: &LoopState<'_>) {
    let Some(kernel) = state.kernel_for(state.active) else {
        return;
    };
    let workspace = kernel.agent_workspace();
    if let Err(e) = tokio::fs::create_dir_all(&workspace).await {
        warn!(path = %workspace.display(), error = %e, "Failed to create agent workspace");
    }
    let _ = state
        .commands
        .send(UiCommand::SetSandboxRoot(workspace))
        .await;
}

pub(super) async fn send_agent_list(state: &LoopState<'_>) {
    state.hub.send_agent_list().await;
}

async fn show_error(state: &LoopState<'_>, message: String) {
    let _ = state.commands.send(UiCommand::ShowError(message)).await;
}

async fn show_success(state: &LoopState<'_>, message: String) {
    let _ = state.commands.send(UiCommand::ShowSuccess(message)).await;
}

#[cfg(test)]
mod check_notice_tests {
    use super::*;
    use aura_agent::CheckCommand;

    fn report(source: CheckSource, outcome: CheckOutcome) -> CheckReport {
        CheckReport {
            command: CheckCommand {
                command: "cargo check".into(),
                source,
            },
            outcome,
        }
    }

    fn text(command: Option<UiCommand>) -> Option<String> {
        match command? {
            UiCommand::ShowWarning(t) | UiCommand::ShowSuccess(t) => Some(t),
            _ => None,
        }
    }

    #[test]
    fn failures_name_the_command_and_its_origin() {
        let failed = CheckOutcome::Failed {
            exit_code: Some(101),
            timed_out: false,
        };
        assert_eq!(
            text(check_notice(&report(CheckSource::AgentsMd, failed), false)).as_deref(),
            Some(
                "Check `cargo check` (from AGENTS.md) failed with exit code 101; the agent was \
                 given the output"
            )
        );
    }

    #[test]
    fn quiet_while_nothing_changes() {
        let pass = CheckOutcome::Passed { recovered: false };
        assert!(check_notice(&report(CheckSource::Env, pass.clone()), true).is_some());
        assert!(check_notice(&report(CheckSource::Env, pass), false).is_none());
        assert!(
            check_notice(&report(CheckSource::Env, CheckOutcome::StillFailing), false).is_none()
        );
        let recovered = CheckOutcome::Passed { recovered: true };
        assert_eq!(
            text(check_notice(&report(CheckSource::Env, recovered), false)).as_deref(),
            Some("Check `cargo check` passes again")
        );
    }
}
