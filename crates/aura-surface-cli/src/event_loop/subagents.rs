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
use super::record_ui::create_response_transaction;
use super::TURN_TIMEOUT;
use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::model_selection::ModelLimits;
use crate::record_loader::agent_summaries;
use async_trait::async_trait;
use aura_agent::{AgentLoop, AgentLoopConfig};
use aura_context_skills::SkillManager;
use aura_core_types::{
    AgentId, SubagentDispatchRequest, SubagentExit, SubagentResult, Transaction,
};
use aura_model_reasoner::{Message, ToolDefinition};
use aura_terminal::events::{AgentDefinitionSummary, AgentSummary};
use aura_terminal::UiCommand;
use aura_tools::agents::task::TASK_TOOL_NAME;
use aura_tools::SubagentDispatchHook;
use std::collections::HashSet;
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
}

/// Parts of an [`AgentHub`] other than the kernel factory.
pub struct AgentHubParts {
    pub registry: AgentRegistry,
    pub definitions: AgentDefinitions,
    pub skills: SkillManager,
    pub base: TurnBase,
    pub commands: mpsc::Sender<UiCommand>,
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
                running: Mutex::new(HashSet::new()),
                finished: Mutex::new(Vec::new()),
                factory: factory(dispatcher),
                commands: parts.commands,
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

    /// Rows for the Permanent agents panel.
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
        let summaries = self.agent_summaries();
        let active = lock(&self.registry).active().agent_id();
        let _ = self.commands.send(UiCommand::SetAgents(summaries)).await;
        let _ = self
            .commands
            .send(UiCommand::SetActiveAgent(active.to_hex()))
            .await;
    }

    /// Send the Permanent agents panel rows to the UI.
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

        let instance = {
            let mut registry = lock(&self.registry);
            let parent = registry
                .find(request.parent_agent_id)
                .map(|a| a.zns_id.clone());
            registry
                .create_instance(&definition.name, parent)
                .map_err(|e| format!("registering {} instance: {e:#}", definition.name))?
        };
        let mut session = self
            .factory
            .open(&instance, request.parent_chain.clone())
            .map_err(|e| format!("opening {}: {e:#}", instance.name))?;
        let child_id = session.agent_id();
        info!(child = %instance.name, definition = %definition.name, "Spawning permanent agent");

        lock(&self.running).insert(child_id);
        self.send_agent_list().await;
        let _ = self
            .commands
            .send(UiCommand::SetStatus(format!(
                "{} is working...",
                instance.name
            )))
            .await;

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
        let outcome = run_task_turn(
            &mut session,
            &AgentLoop::new(config),
            tools,
            &request.prompt,
        )
        .await;

        lock(&self.running).remove(&child_id);
        lock(&self.finished).push(session);
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

/// Record `prompt` on `session` and run one turn of it, returning the
/// final reply.
async fn run_task_turn(
    session: &mut AgentSession,
    agent_loop: &AgentLoop,
    tools: Vec<ToolDefinition>,
    prompt: &str,
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

    let cancel = CancellationToken::new();
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
