//! Spawning permanent agents from a turn, and the agent state the event
//! loop shares with that spawning.
//!
//! A `task` call names a permanent agent. The dispatcher registers a new
//! instance under the calling agent, runs one turn of it with the
//! definition's rules, tools and model, and returns its final reply as the
//! tool result. The instance stays in the agents list so the user can open
//! it and keep talking to it.
//!
//! Spawns run inside the parent's tool call, on the parent's turn task, so
//! the event loop keeps running beside them and the user can open a spawn
//! to watch it work. Everything they touch lives in [`AgentHub`] behind
//! short-lived locks that are never held across an `.await`.

use super::agent_events::AgentUi;
use super::agents::{AgentSession, KernelFactory};
use super::handlers::{handle_agent_success, run_agent_turn};
use super::steering::Steering;
use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::long_run::LongRunSettings;
use crate::model_selection::ModelLimits;
use crate::project_instructions::ProjectInstructions;
use crate::record_loader::agent_summaries;
use async_trait::async_trait;
use aura_agent::{AgentLoop, AgentLoopConfig};
use aura_agent_kernel::Kernel;
use aura_context_skills::SkillManager;
use aura_core_types::{
    AgentId, SubagentDispatchRequest, SubagentExit, SubagentResult, Transaction,
};
use aura_model_reasoner::{Message, ToolDefinition};
use aura_terminal::events::{AgentDefinitionSummary, AgentSummary};
use aura_terminal::{PermissionMode, UiCommand};
use aura_tools::agents::task::TASK_TOOL_NAME;
use aura_tools::jobs::{JOB_KILL_TOOL_NAME, JOB_OUTPUT_TOOL_NAME};
use aura_tools::todo::TODO_WRITE_TOOL_NAME;
use aura_tools::SubagentDispatchHook;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// An agent with work in flight.
struct Running {
    kernel: Arc<Kernel>,
    cancel: CancellationToken,
    /// Input for its turn; `None` while it runs a typed shell command.
    steering: Option<Steering>,
}

/// What every turn starts from before a permanent agent's settings apply.
pub struct TurnBase {
    /// The terminal's agent-loop config (selected model, auth, prompt).
    pub config: AgentLoopConfig,
    /// Built-in tools, without `task`.
    pub tools: Vec<ToolDefinition>,
    /// For running a permanent agent on its own model.
    pub limits: ModelLimits,
}

/// Agent state shared by the event loop and the subagent dispatcher.
pub struct AgentHub {
    pub(super) registry: Mutex<AgentRegistry>,
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
    /// Permission mode of each agent the user or a parent set one for
    modes: Mutex<HashMap<AgentId, PermissionMode>>,
    /// Mode of every other agent
    default_mode: PermissionMode,
    /// `AGENTS.md`, `CLAUDE.md` and rules for each working folder.
    pub(crate) instructions: ProjectInstructions,
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
    pub instructions: ProjectInstructions,
}

pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl AgentHub {
    /// Build the hub. `factory` receives the dispatcher that kernels must
    /// install so their `task` calls spawn permanent agents.
    pub fn new(
        parts: AgentHubParts,
        factory: impl FnOnce(Arc<dyn SubagentDispatchHook>) -> KernelFactory,
    ) -> Arc<Self> {
        Arc::new_cyclic(|hub: &Weak<Self>| {
            let dispatcher: Arc<dyn SubagentDispatchHook> =
                Arc::new(TerminalSubagentDispatcher { hub: hub.clone() });
            Self {
                registry: Mutex::new(parts.registry),
                definitions: Mutex::new(parts.definitions),
                skills: Mutex::new(parts.skills),
                base: Mutex::new(parts.base),
                running: Mutex::new(HashMap::new()),
                finished: Mutex::new(Vec::new()),
                spawn_finished: Notify::new(),
                factory: factory(dispatcher),
                commands: parts.commands,
                long_run: parts.long_run,
                modes: Mutex::new(HashMap::new()),
                default_mode: parts.mode,
                instructions: parts.instructions,
            }
        })
    }

    /// Permission mode of `agent_id`.
    pub fn mode_of(&self, agent_id: AgentId) -> PermissionMode {
        lock(&self.modes)
            .get(&agent_id)
            .copied()
            .unwrap_or(self.default_mode)
    }

    /// Note that `agent_id` is in `mode` from now on.
    pub(super) fn set_mode(&self, agent_id: AgentId, mode: PermissionMode) {
        lock(&self.modes).insert(agent_id, mode);
    }

    /// Names of every tool a turn may offer, for the mode tables.
    pub(crate) fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = lock(&self.base)
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        names.push(TASK_TOOL_NAME.to_string());
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

    /// Send the Agents panel rows and the active agent to the UI.
    pub async fn send_agent_list(&self) {
        let summaries = self.agent_summaries();
        let active = lock(&self.registry).active().agent_id();
        let _ = self.commands.send(UiCommand::SetAgents(summaries)).await;
        let _ = self
            .commands
            .send(UiCommand::SetActiveAgent(active.to_hex()))
            .await;
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
    pub(super) fn start_running(
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
            },
        );
    }

    /// Input for the turn `agent_id` is running, if it is running one.
    pub(super) fn steering(&self, agent_id: AgentId) -> Option<Steering> {
        lock(&self.running)
            .get(&agent_id)
            .and_then(|running| running.steering.clone())
    }

    /// Note that `agent_id` has no work in flight any more.
    pub(super) fn stop_running(&self, agent_id: AgentId) {
        lock(&self.running).remove(&agent_id);
    }

    /// Whether `agent_id` has work in flight.
    pub(super) fn is_running(&self, agent_id: AgentId) -> bool {
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

    async fn spawn(&self, request: SubagentDispatchRequest) -> Result<SubagentResult, String> {
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
        let mut session = self
            .factory
            .open(&instance, request.parent_chain.clone(), folder)
            .map_err(|e| format!("opening {}: {e:#}", instance.name))?;
        let child_id = session.agent_id();
        info!(child = %instance.name, definition = %definition.name, "Spawning permanent agent");
        // A spawn asks no less than the agent that started it.
        let mode = self.mode_of(request.parent_agent_id);
        self.set_mode(child_id, mode);
        super::modes::apply(&session.kernel, mode, &self.tool_names());

        let cancel = CancellationToken::new();
        // Messages for a spawn wait for its task to end: steering it
        // mid-task would change the work its parent is waiting on.
        self.start_running(child_id, Arc::clone(&session.kernel), cancel.clone(), None);
        self.send_agent_list().await;

        let workspace = session.kernel.agent_workspace();
        self.send_instructions(child_id, &workspace).await;
        let (mut config, tools) = self.turn(Some(&definition.name), &workspace);
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
        let ui = AgentUi::new(self.commands.clone(), child_id);
        let outcome = run_task_turn(
            &mut session,
            &AgentLoop::new(config),
            tools,
            &request.prompt,
            &ui,
            &cancel,
            self.long_run,
        )
        .await;

        // Parked before it stops running, so the event loop never sees the
        // agent with neither a session nor work in flight.
        lock(&self.finished).push(session);
        self.stop_running(child_id);
        self.spawn_finished.notify_one();
        self.send_agent_list().await;

        Ok(match outcome {
            Ok(text) => SubagentResult::completed(child_id, text),
            Err(reason) => {
                warn!(child = %instance.name, %reason, "Permanent agent failed");
                SubagentResult {
                    child_agent_id: Some(child_id),
                    final_message: String::new(),
                    total_input_tokens: 0,
                    total_output_tokens: 0,
                    files_changed: Vec::new(),
                    exit: SubagentExit::Failed { reason },
                }
            }
        })
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
    let catalog: Vec<String> = definitions
        .all()
        .iter()
        .map(|d| format!("- {}: {}", d.name, d.description))
        .collect();
    Some(ToolDefinition::new(
        TASK_TOOL_NAME,
        format!(
            "Spawn a permanent agent to do a task and wait for its final reply. \
             It works in the same workspace but with its own rules, tools and \
             context, so give it a complete, self-contained prompt.\n\
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
                }
            },
            "required": ["subagent_type", "prompt"]
        }),
    ))
}

/// Record `prompt` on `session` and run one turn of it, streaming it into
/// `ui`, and return the final reply.
async fn run_task_turn(
    session: &mut AgentSession,
    agent_loop: &AgentLoop,
    tools: Vec<ToolDefinition>,
    prompt: &str,
    ui: &AgentUi,
    cancel: &CancellationToken,
    limits: LongRunSettings,
) -> Result<String, String> {
    let agent_id = session.agent_id();
    let store = session.kernel.store();
    store
        .enqueue_tx(&Transaction::user_prompt(agent_id, prompt.to_string()))
        .map_err(|e| format!("queueing prompt: {e}"))?;
    let (token, tx) = store
        .dequeue_tx(agent_id)
        .map_err(|e| format!("dequeueing prompt: {e}"))?
        .ok_or_else(|| "prompt was not queued".to_string())?;
    session
        .kernel
        .process_dequeued(tx, token)
        .await
        .map_err(|e| format!("recording prompt: {e}"))?;
    session.messages.push(Message::user(prompt));
    ui.send(UiCommand::TurnStarted {
        prompt: Some(prompt.to_string()),
    })
    .await;

    // Nothing sends to a spawn mid-task, so its steering stays empty.
    let steering = Steering::new(cancel);
    let (result, forwarded) =
        run_agent_turn(session, agent_loop, tools, ui, cancel, &steering, limits).await;
    steering.finish(&[]);
    let result = match result {
        Ok(result) => result,
        Err(e) => {
            let reason = if cancel.is_cancelled() {
                "cancelled by the user".to_string()
            } else {
                e.to_string()
            };
            ui.send(UiCommand::ShowWarning(reason.clone())).await;
            ui.send(UiCommand::Complete).await;
            return Err(reason);
        }
    };
    let text = result.total_text.clone();
    let llm_error = result.llm_error.as_ref().map(ToString::to_string);
    handle_agent_success(session, result, &forwarded, ui, &[]).await;
    match llm_error {
        _ if cancel.is_cancelled() => Err("cancelled by the user".to_string()),
        Some(err) if text.trim().is_empty() => Err(err),
        _ => Ok(text),
    }
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
