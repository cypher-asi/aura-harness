//! Spawning permanent agents from a turn, and the agent state the event
//! loop shares with that spawning.
//!
//! A `task` call names a permanent agent. The dispatcher registers a new
//! instance under the calling agent, runs one turn of it with the
//! definition's rules, tools and model, and returns its final reply as the
//! tool result. The instance stays in the agents list so the user can open
//! it and keep talking to it.
//!
//! Spawns run inside the parent's tool call, so the event loop is blocked
//! for their duration; everything they touch lives in [`AgentHub`] behind
//! short-lived locks that are never held across an `.await`.

use super::agents::{AgentSession, KernelFactory};
use super::host::{agent_tool_definitions, TerminalAgents};
use super::notices::{Notice, NoticeSender};
use super::processes::{now_ms, task_id, ProcessTable};
use super::record_ui::{create_response_transaction, RecordPanelAgent};
use super::TURN_TIMEOUT;
use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::model_selection::ModelLimits;
use crate::record_loader::agent_summaries;
use async_trait::async_trait;
use aura_agent::{AgentLoop, AgentLoopConfig};
use aura_context_skills::SkillManager;
use aura_core_types::{
    AgentId, SpawnMode, SubagentDispatchRequest, SubagentExit, SubagentResult, Transaction,
};
use aura_model_reasoner::{Message, ToolDefinition};
use aura_terminal::events::{
    AgentDefinitionSummary, AgentSummary, ProcessKind, ProcessStatus, ProcessSummary,
};
use aura_terminal::UiCommand;
use aura_tools::agents::task::TASK_TOOL_NAME;
use aura_tools::SubagentDispatchHook;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

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
    /// Instances working on a task for their parent.
    running: Mutex<HashSet<AgentId>>,
    /// Sessions of finished spawns, waiting for the event loop to park them.
    finished: Mutex<Vec<AgentSession>>,
    pub(crate) factory: KernelFactory,
    commands: mpsc::Sender<UiCommand>,
    pub(crate) processes: Arc<ProcessTable>,
    /// Messages for agents from work that finishes off the event loop.
    pub(crate) notices: NoticeSender,
    notice_rx: Mutex<Option<mpsc::UnboundedReceiver<Notice>>>,
    pub(crate) agents: Option<TerminalAgents>,
    /// The agent the RECORD panel follows.
    pub(crate) record_panel: RecordPanelAgent,
}

/// Parts of an [`AgentHub`] other than the kernel factory.
pub struct AgentHubParts {
    pub registry: AgentRegistry,
    pub definitions: AgentDefinitions,
    pub skills: SkillManager,
    pub base: TurnBase,
    pub commands: mpsc::Sender<UiCommand>,
    /// Lists spawns in the Processes tab.
    pub processes: Arc<ProcessTable>,
    /// Runs the agents `spawn_agent` starts; `None` leaves the tool out.
    pub agents: Option<TerminalAgents>,
    /// Shared with the kernels' [`super::UiRecordObserver`].
    pub record_panel: RecordPanelAgent,
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
        let (notices, notice_rx) = mpsc::unbounded_channel();
        Arc::new_cyclic(|hub: &Weak<Self>| {
            let dispatcher: Arc<dyn SubagentDispatchHook> =
                Arc::new(TerminalSubagentDispatcher { hub: hub.clone() });
            Self {
                registry: Mutex::new(parts.registry),
                definitions: Mutex::new(parts.definitions),
                skills: Mutex::new(parts.skills),
                base: Mutex::new(parts.base),
                running: Mutex::new(HashSet::new()),
                finished: Mutex::new(Vec::new()),
                factory: factory(dispatcher),
                commands: parts.commands,
                processes: parts.processes,
                notices,
                notice_rx: Mutex::new(Some(notice_rx)),
                agents: parts.agents,
                record_panel: parts.record_panel,
            }
        })
    }

    /// Config and tools for a turn of an agent, applying the permanent
    /// agent `definition` when it names one.
    pub fn turn(&self, definition: Option<&str>) -> (AgentLoopConfig, Vec<ToolDefinition>) {
        let base = lock(&self.base);
        let definitions = lock(&self.definitions);
        let mut config = base.config.clone();
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
        tools = definition.filter_tools(&tools);
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
        let running = lock(&self.running).clone();
        agent_summaries(
            &registry,
            self.factory.store.as_ref(),
            registry.active().agent_id(),
            &running,
        )
    }

    /// Send the Agents panel rows and the active agent to the UI.
    pub async fn send_agent_list(&self) {
        self.name_agents();
        let summaries = self.agent_summaries();
        let active = lock(&self.registry).active().agent_id();
        let _ = self.commands.send(UiCommand::SetAgents(summaries)).await;
        let _ = self
            .commands
            .send(UiCommand::SetActiveAgent(active.to_hex()))
            .await;
    }

    /// Let `agent`, about to run a turn on `config` in `folder`, spawn
    /// background agents, and list them in the Processes tab.
    pub(crate) fn register_root(
        &self,
        agent: AgentId,
        name: &str,
        folder: PathBuf,
        config: &AgentLoopConfig,
    ) {
        if let Some(agents) = &self.agents {
            agents.register(agent, name, folder, config, self.notices.clone());
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
            self.processes.name_agent(agent.agent_id(), &agent.name);
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

        let detached = request.spawn_mode == Some(SpawnMode::Detached);
        lock(&self.running).insert(child_id);
        self.send_agent_list().await;
        if !detached {
            let _ = self
                .commands
                .send(UiCommand::SetStatus(format!(
                    "{} is working...",
                    instance.name
                )))
                .await;
        }

        let (mut config, tools) = self.turn(Some(&definition.name));
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
        self.register_root(
            child_id,
            &instance.name,
            session.kernel.agent_workspace(),
            &config,
        );
        let cancel = CancellationToken::new();
        let mut summary = task_summary(
            &request,
            &instance.name,
            &definition.name,
            child_id,
            &config.model,
        );
        if detached {
            summary.kind = ProcessKind::Agent;
        }
        self.processes.start(summary, Some(cancel.clone()));
        let run = TaskRun {
            session,
            agent_loop: AgentLoop::new(config),
            tools,
            prompt: request.prompt,
            cancel,
        };

        if detached {
            info!(child = %instance.name, "Running permanent agent in the background");
            let hub = Arc::clone(self);
            let parent = request.parent_agent_id;
            let name = instance.name.clone();
            tokio::spawn(async move {
                let outcome = hub.finish_task(run).await;
                let _ = hub.notices.send(task_notice(parent, &name, outcome));
            });
            return Ok(SubagentResult::completed(
                child_id,
                format!(
                    "{} is working on this in the background. Its reply will arrive \
                     as a message when it finishes; carry on meanwhile.",
                    instance.name
                ),
            ));
        }

        Ok(match self.finish_task(run).await {
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

    /// Run `run`'s turn to the end, then hand its session back for the
    /// event loop to keep.
    async fn finish_task(&self, mut run: TaskRun) -> Result<String, String> {
        let child_id = run.session.agent_id();
        let outcome = run_task_turn(
            &mut run.session,
            &run.agent_loop,
            run.tools,
            &run.prompt,
            run.cancel.clone(),
        )
        .await;
        let outcome = if run.cancel.is_cancelled() {
            Err("stopped from the Processes tab".to_string())
        } else {
            outcome
        };
        self.processes.finish(
            &task_id(child_id),
            if outcome.is_ok() {
                ProcessStatus::Exited
            } else {
                ProcessStatus::Failed
            },
        );

        lock(&self.running).remove(&child_id);
        lock(&self.finished).push(run.session);
        self.send_agent_list().await;
        outcome
    }

    /// Whether `agent` is doing a task, so its session is in use.
    pub(crate) fn is_running(&self, agent: AgentId) -> bool {
        lock(&self.running).contains(&agent)
    }
}

/// A spawned permanent agent's turn, ready to run.
struct TaskRun {
    session: AgentSession,
    agent_loop: AgentLoop,
    tools: Vec<ToolDefinition>,
    prompt: String,
    cancel: CancellationToken,
}

/// What `parent` is told when its background agent `name` finishes.
fn task_notice(parent: AgentId, name: &str, outcome: Result<String, String>) -> Notice {
    match outcome {
        Ok(reply) => Notice {
            to: parent,
            shown: format!("{name} finished its background task"),
            prompt: format!(
                "{name} finished the task you gave it in the background. Its reply:\n\n{reply}"
            ),
        },
        Err(reason) => Notice {
            to: parent,
            shown: format!("{name} did not finish its background task: {reason}"),
            prompt: format!(
                "{name} did not finish the task you gave it in the background: {reason}"
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
             context, so give it a complete, self-contained prompt. With \
             spawn_mode `detached` the call returns at once and the agent's \
             reply arrives later as a message, so you can keep working or \
             start several agents side by side.\n\
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
                }
            },
            "required": ["subagent_type", "prompt"]
        }),
    ))
}

/// Processes tab row for the `task` spawn running as `child`.
fn task_summary(
    request: &SubagentDispatchRequest,
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
        detail: request.prompt.clone(),
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

/// Record `prompt` on `session` and run one turn of it, returning the
/// final reply.
async fn run_task_turn(
    session: &mut AgentSession,
    agent_loop: &AgentLoop,
    tools: Vec<ToolDefinition>,
    prompt: &str,
    cancel: CancellationToken,
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

    let result = match tokio::time::timeout(
        TURN_TIMEOUT,
        agent_loop.run_with_events(
            &session.model_gateway,
            &session.tool_gateway,
            session.messages.clone(),
            tools,
            None,
            Some(cancel.clone()),
        ),
    )
    .await
    {
        Ok(result) => result.map_err(|e| e.to_string())?,
        Err(_) => {
            cancel.cancel();
            return Err(format!(
                "timed out after {} minutes",
                TURN_TIMEOUT.as_secs() / 60
            ));
        }
    };
    session.messages.clone_from(&result.messages);

    let response = create_response_transaction(agent_id, &result.total_text);
    if let Err(e) = session.kernel.process_direct(response).await {
        warn!(error = %e, "Failed to record permanent agent reply");
    }
    match result.llm_error {
        Some(err) if result.total_text.trim().is_empty() => Err(err.to_string()),
        _ => Ok(result.total_text),
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
