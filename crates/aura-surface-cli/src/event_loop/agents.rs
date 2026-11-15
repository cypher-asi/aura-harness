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
use super::LoopState;
use crate::agent_registry::RegisteredAgent;
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
use aura_model_reasoner::{ContentBlock, Message};
use aura_store_db::ReadStore;
use aura_terminal::UiCommand;
use aura_tools::todo::{apply_todo_write, TodoItem, TODO_WRITE_TOOL_NAME};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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

    /// Continue the session saved in `checkpoint`.
    pub fn restore(&mut self, checkpoint: SessionCheckpoint) {
        self.messages = checkpoint.messages;
        if !is_resumable(&self.messages) {
            self.messages.pop();
        }
        self.todos = checkpoint.todos;
        self.usage = checkpoint.usage;
        self.started_ms = checkpoint.started_ms;
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
            self.usage,
        );
        if let Err(e) = checkpoints.save(agent_id, &checkpoint).await {
            warn!(%agent_id, error = %e, "Failed to save session checkpoint");
        }
    }
}

/// The todo list left by the `todo_write` calls in `messages`.
pub(crate) fn todos_from_history(messages: &[Message]) -> Vec<TodoItem> {
    let mut todos = Vec::new();
    for block in messages.iter().flat_map(|m| m.content.iter()) {
        if let ContentBlock::ToolUse { name, input, .. } = block {
            if name == TODO_WRITE_TOOL_NAME {
                if let Ok(next) = apply_todo_write(&todos, input) {
                    todos = next;
                }
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
    pub(super) store: Arc<dyn ReadStore>,
    build_kernel: KernelBuilder,
    pub(super) checkpoints: Option<Checkpoints>,
    pub(super) file_checkpoints: Option<FileCheckpoints>,
    post_edit_check: Option<(PostEditCheckConfig, mpsc::Sender<UiCommand>)>,
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
        }
    }

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
        let kernel = Arc::new((self.build_kernel)(
            identity.agent_id,
            parent_chain,
            folder,
        )?);
        let post_edit_check = self.post_edit_check.as_ref().map(|(config, commands)| {
            PostEditCheck::new(kernel.agent_workspace(), config.clone())
                .with_notifier(check_notifier(commands.clone(), identity.agent_id))
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

pub(super) async fn handle_create_project(state: &mut LoopState<'_>, name: &str, folder: &Path) {
    let created = lock(&state.hub.registry).create_project(name, folder);
    match created {
        Ok(agent) => {
            let project = agent
                .project
                .clone()
                .unwrap_or_else(|| name.trim().to_string());
            let folder = agent
                .folder
                .as_deref()
                .unwrap_or(folder)
                .display()
                .to_string();
            info!(zns_id = %agent.zns_id, %project, %folder, "Created project");
            activate_agent(state, agent.agent_id(), Chat::Reload).await;
            show_success(state, format!("Created project {project} in {folder}")).await;
        }
        Err(e) => show_error(state, format!("Could not create project: {e}")).await,
    }
}

pub(super) async fn handle_rename_project(state: &mut LoopState<'_>, name: &str) {
    let agent_id = lock(&state.hub.registry).active().agent_id();
    let renamed = lock(&state.hub.registry).rename_project(agent_id, name);
    match renamed {
        Ok(()) => {
            state.hub.send_agent_list().await;
            show_success(state, format!("Renamed project to {}", name.trim())).await;
        }
        Err(e) => show_error(state, format!("Could not rename project: {e}")).await,
    }
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

/// Start an instance of a permanent agent and switch to it.
pub(super) async fn handle_instantiate_definition(state: &mut LoopState<'_>, name: &str) {
    if let Some(agent) = start_instance(state, name, None).await {
        show_success(state, format!("Started {}", agent.name)).await;
    }
}

/// Start an instance of a permanent agent inside the project that contains
/// the active agent, and switch to it.
pub(super) async fn handle_add_definition_to_project(state: &mut LoopState<'_>, name: &str) {
    let root = lock(&state.hub.registry)
        .project_root(state.active)
        .cloned();
    let Some(root) = root else {
        show_error(state, "Unknown agent".to_string()).await;
        return;
    };
    if let Some(agent) = start_instance(state, name, Some(root.zns_id.clone())).await {
        let project = root
            .project
            .as_deref()
            .map(str::trim)
            .filter(|project| !project.is_empty())
            .unwrap_or(&root.name);
        show_success(state, format!("Added {} to {project}", agent.name)).await;
    }
}

/// Register an instance of the permanent agent `name` under `parent` and
/// switch to it. Reports failures itself and returns `None`.
async fn start_instance(
    state: &mut LoopState<'_>,
    name: &str,
    parent: Option<String>,
) -> Option<RegisteredAgent> {
    let definition = lock(&state.hub.definitions)
        .get(name)
        .map(|d| d.name.clone());
    let Some(definition) = definition else {
        show_error(state, format!("No permanent agent named {name}")).await;
        return None;
    };
    let created = lock(&state.hub.registry).create_instance(&definition, parent);
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
async fn activate_agent(state: &mut LoopState<'_>, agent_id: AgentId, chat: Chat) {
    state.park_finished_spawns().await;
    if agent_id == state.active {
        state.hub.send_agent_list().await;
        return;
    }
    if !state.idle.contains_key(&agent_id) && !state.hub.is_running(agent_id) {
        match open_session(state, agent_id) {
            Ok(session) => {
                state.idle.insert(agent_id, session);
            }
            Err(problem) => {
                show_error(state, problem).await;
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
    let _ = state.commands.send(UiCommand::ClearRecords).await;
    match recent_record_summaries(store, agent_id) {
        Ok(summaries) => {
            for summary in summaries {
                let _ = state.commands.send(UiCommand::NewRecord(summary)).await;
            }
        }
        Err(e) => {
            let _ = state
                .commands
                .send(UiCommand::ShowWarning(format!(
                    "Could not load record history: {e}"
                )))
                .await;
        }
    }

    if chat == Chat::Reload && !state.hub.is_running(agent_id) {
        let history = conversation_history(store, agent_id).unwrap_or_else(|e| {
            warn!(error = %e, "Failed to rebuild conversation from record");
            Vec::new()
        });
        AgentUi::new(state.commands.clone(), agent_id)
            .send(UiCommand::LoadConversation(history))
            .await;
        super::hud::send_session_of(state, agent_id).await;
    }
    super::modes::send_mode(state, agent_id).await;
    super::hud::send_instructions_of(state, agent_id).await;
    if state.hub.is_running(agent_id) {
        // Its turn shows its plan again when it next changes.
        let _ = state.commands.send(UiCommand::SetPlan(None)).await;
    } else {
        super::plan_mode::send_plan_of(state, agent_id).await;
    }

    send_sandbox_root(state).await;
    state.hub.send_agent_list().await;
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
