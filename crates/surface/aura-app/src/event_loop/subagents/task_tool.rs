use super::*;

/// What `workspace` holds now, when it is in a git repository, so a
/// failed attempt's changes (shell commands' included) can be undone.
pub(super) async fn snapshot_workspace(workspace: PathBuf) -> Option<changes::Snapshot> {
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
pub(super) async fn restore_workspace(snapshot: changes::Snapshot) {
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
pub(super) fn reads_only(definition: &AgentDefinition) -> bool {
    definition
        .tools
        .as_ref()
        .is_some_and(|tools| tools.iter().all(|tool| aura_gate::lead::keeps_tool(tool)))
}

/// What a subtask is told `left` before its time is up.
pub(super) fn wrap_up_notice(left: Duration) -> String {
    format!(
        "About {} minutes are left for this task. Start no new checks: reply now with \
         what you found, and say what you did not get to.",
        left.as_secs() / 60
    )
}

/// What `parent` is told when its background agent `name` finishes.
pub(super) fn task_notice(
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
            inbox: None,
        },
        Err(reason) => Notice {
            to: parent,
            shown: format!("{name} did not finish its background task: {reason}"),
            prompt: format!(
                "{name} did not finish the task you gave it in the background: {reason}{}",
                report(false)
            ),
            inbox: None,
        },
    }
}

/// `task` tool schema listing the current permanent agents, or `None` when
/// there are none to spawn. The channel coordinator is never listed: only
/// channels create its instances.
#[must_use]
pub fn task_tool_definition(definitions: &AgentDefinitions) -> Option<ToolDefinition> {
    let spawnable: Vec<&AgentDefinition> = definitions
        .all()
        .iter()
        .filter(|d| !d.is_coordinator())
        .collect();
    if spawnable.is_empty() {
        return None;
    }
    let names: Vec<&str> = spawnable.iter().map(|d| d.name.as_str()).collect();
    let catalog: Vec<String> = spawnable.iter().map(|d| catalog_entry(d)).collect();
    Some(ToolDefinition::new(
        TASK_TOOL_NAME,
        task_tool_description(&catalog),
        task_tool_schema(&names),
    ))
}

/// The `task` tool's description, ending with the agents' `catalog`.
fn task_tool_description(catalog: &[String]) -> String {
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
    )
}

/// The `task` tool's input schema, offering the agents `names`.
fn task_tool_schema(names: &[&str]) -> serde_json::Value {
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
            "description": {
                "type": "string",
                "description": aura_exec_tools::agents::TASK_DESCRIPTION_HELP
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
    })
}

/// `- name: description (model: m; tools: a, b)`: one agent of the `task`
/// tool's catalog, with what it runs on when that is set.
pub(super) fn catalog_entry(definition: &AgentDefinition) -> String {
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

/// `assign` tool schema for a channel coordinator, listing the permanent
/// agents a new member can be an instance of, or `None` when there are
/// none.
#[must_use]
pub fn assign_tool_definition(definitions: &AgentDefinitions) -> Option<ToolDefinition> {
    let roles: Vec<&AgentDefinition> = definitions
        .all()
        .iter()
        .filter(|d| !d.is_coordinator())
        .collect();
    if roles.is_empty() {
        return None;
    }
    let names: Vec<&str> = roles.iter().map(|d| d.name.as_str()).collect();
    let catalog: Vec<String> = roles.iter().map(|d| catalog_entry(d)).collect();
    Some(ToolDefinition::new(
        ASSIGN_TOOL_NAME,
        format!(
            "Hand a piece of the channel's work to an agent: a member of the channel \
             in `to`, by its code, or a new member started from a permanent agent in \
             `agent` when no member fits or is free. The call returns at once; the agent \
             works on its own and its report arrives in your inbox when it finishes. \
             Brief the goal, the context and what done looks like. How the agent works, \
             subagents included, is its call.\n\
             Permanent agents:\n{}",
            catalog.join("\n")
        ),
        assign_tool_schema(&names),
    ))
}

/// The `assign` tool's input schema, offering the permanent agents
/// `names` to start new members from.
fn assign_tool_schema(names: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "to": {
                "type": "string",
                "description": "Code of the member to give the work to, such as `REN4`."
            },
            "agent": {
                "type": "string",
                "enum": names,
                "description": "Permanent agent to start a new member from."
            },
            "subject": {
                "type": "string",
                "description": "A few words naming the work, shown in inboxes and the channel."
            },
            "prompt": {
                "type": "string",
                "description": "The brief: the goal, the context and what done looks like."
            },
            "todo_id": {
                "type": "string",
                "description": "Id of the todo this work does. The todo shows the agent and goes in progress now; the agent's report marks it done, or pending again when the work failed. Giving a todo to another member takes it back from the one that had it, if it hadn't started."
            },
            "withdraw": {
                "type": "boolean",
                "description": "Take back the work given to the member in `to` that it hasn't started, instead of assigning more: it won't start. Work already under way is not stopped. `subject` and `prompt` may then say why."
            }
        },
        "required": ["subject", "prompt"]
    })
}

/// The tools out of `tools` an instance of `definition` gets: the ones
/// its file allows, and the ones it never needs to name. Planning and job
/// tools only touch the agent's own state. A top-level agent also keeps
/// `task` and its inbox: whether it hands work to subagents or settles
/// details with its peers is its own call.
pub(super) fn definition_tools(
    definition: &AgentDefinition,
    tools: &[ToolDefinition],
    top_level: bool,
) -> Vec<ToolDefinition> {
    let mut kept = definition.filter_tools(tools);
    let runs_commands = kept.iter().any(|t| t.name == "run_command");
    let always = |tool: &ToolDefinition| {
        tool.name == TODO_WRITE_TOOL_NAME
            || (runs_commands
                && (tool.name == JOB_OUTPUT_TOOL_NAME || tool.name == JOB_KILL_TOOL_NAME))
            || (top_level && (tool.name == TASK_TOOL_NAME || is_inbox_tool(&tool.name)))
    };
    let extra: Vec<ToolDefinition> = tools
        .iter()
        .filter(|tool| always(tool) && !kept.iter().any(|t| t.name == tool.name))
        .cloned()
        .collect();
    kept.extend(extra);
    kept
}

/// Processes tab row for the `task` spawn running as `child`, asked
/// `prompt`.
pub(super) fn task_summary(
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

/// Runs `task` calls by spawning permanent agents.
pub(super) struct TerminalSubagentDispatcher {
    pub(super) hub: Weak<AgentHub>,
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
