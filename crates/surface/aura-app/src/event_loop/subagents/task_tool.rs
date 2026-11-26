use super::*;
use crate::event_loop::agent_events::ForwarderState;

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

/// How a subtask's turn ended.
pub(super) struct TaskEnd {
    /// The final reply, or why there is none
    pub(super) reply: Result<String, String>,
    /// The recorded prompt and its category, when Gate routed the
    /// subtask and recorded it
    pub(super) prompt: Option<(Hash, PromptCategory)>,
    /// What the turn's end says about how it went; `None` when nothing
    /// does, and the judge has to
    pub(super) signal: Option<OutcomeSignal>,
    /// List-price cost of the turn's model calls
    pub(super) cost_usd: f64,
    /// Files the turn's file tools changed
    pub(super) files: Vec<String>,
    /// The commands the turn ran and its last test or build, as the
    /// judge of unchecked work is shown them
    pub(super) shown: (String, String),
}

impl TaskEnd {
    pub(super) const fn failed(reason: String) -> Self {
        Self {
            reply: Err(reason),
            prompt: None,
            signal: Some(OutcomeSignal::Error),
            cost_usd: 0.0,
            files: Vec::new(),
            shown: (String::new(), String::new()),
        }
    }
}

/// One turn of a subtask: `prompt`, recorded as `label` says, run on
/// `agent_loop` with `tools` and streamed into `ui`. A `label` Gate could
/// not give ends the turn before it starts.
pub(super) struct TaskTurn<'a> {
    pub(super) agent_loop: &'a AgentLoop,
    pub(super) tools: Vec<ToolDefinition>,
    pub(super) prompt: &'a str,
    pub(super) label: Result<SubtaskLabel, String>,
    pub(super) ui: &'a AgentUi,
    pub(super) cancel: &'a CancellationToken,
    pub(super) limits: LongRunSettings,
    pub(super) recorder: Option<Arc<RunRecorder>>,
}

/// Record `turn`'s prompt on `session` as its label says and run one
/// turn of it, streaming it into its UI.
pub(super) async fn run_task_turn(session: &mut AgentSession, turn: TaskTurn<'_>) -> TaskEnd {
    let turn_start = session.messages.len();
    let recorded = match turn.open(session).await {
        Ok(recorded) => recorded,
        Err(end) => return end,
    };
    let ending = TurnEnding {
        ui: turn.ui,
        cancel: turn.cancel,
        turn_start,
        limit: turn.limits.turn_timeout,
    };
    let (result, forwarded) = turn.run(session).await;
    ending.conclude(session, result, &forwarded, recorded).await
}

impl TaskTurn<'_> {
    /// Record the prompt on `session` as the label says, add it to the
    /// conversation and show the turn starting; the turn's end when there
    /// is no label or the prompt could not be recorded.
    async fn open(
        &self,
        session: &mut AgentSession,
    ) -> Result<Option<(Hash, PromptCategory)>, TaskEnd> {
        let label = match &self.label {
            Ok(label) => label,
            Err(problem) => {
                self.ui.send(UiCommand::ShowError(problem.clone())).await;
                self.ui.send(UiCommand::Complete).await;
                return Err(TaskEnd::failed(problem.clone()));
            }
        };
        let recorded = match label.record(session).await {
            Ok(recorded) => recorded,
            Err(problem) => return Err(TaskEnd::failed(format!("recording prompt: {problem}"))),
        };
        session.messages.push(Message::user(self.prompt));
        self.ui
            .send(UiCommand::TurnStarted {
                prompt: Some(self.prompt.to_string()),
            })
            .await;
        if let Some(line) = &label.shown {
            self.ui.send(system_message(line.clone())).await;
        }
        Ok(recorded)
    }

    /// Run the turn on `session`, told to wrap up shortly before its time
    /// is up, and close it on its trace.
    async fn run(
        self,
        session: &mut AgentSession,
    ) -> (
        Result<aura_agent::AgentLoopResult, aura_agent::AgentError>,
        ForwarderState,
    ) {
        if let Some(rec) = &self.recorder {
            rec.begin_turn(self.prompt);
        }
        // Only the harness sends to a spawn mid-task: that its time is
        // nearly up.
        let steering = Steering::new(self.cancel);
        let wrap_up = schedule_wrap_up(&self.limits, &steering);
        let (result, forwarded) = run_agent_turn(
            session,
            self.agent_loop,
            self.tools,
            self.ui,
            self.cancel,
            &steering,
            self.limits,
            self.recorder.clone(),
        )
        .await;
        if let Some(wrap_up) = wrap_up {
            wrap_up.abort();
        }
        steering.finish(&[]);
        if let Some(rec) = &self.recorder {
            crate::event_loop::trace::end_turn(rec, &result, self.cancel.is_cancelled());
        }
        (result, forwarded)
    }
}

/// Tell the turn through `steering` to wrap up shortly before its time,
/// when `limits` gives it enough for that to matter.
fn schedule_wrap_up(
    limits: &LongRunSettings,
    steering: &Steering,
) -> Option<tokio::task::JoinHandle<()>> {
    limits
        .turn_timeout
        .filter(|limit| *limit > WRAP_UP_BEFORE * 2)
        .map(|limit| {
            let steering = steering.clone();
            tokio::spawn(async move {
                tokio::time::sleep(limit - WRAP_UP_BEFORE).await;
                let notice = wrap_up_notice(WRAP_UP_BEFORE);
                steering
                    .send(SteerSource::Harness, notice.clone(), notice)
                    .await;
            })
        })
}

/// Why a turn that failed with `e` has no reply.
fn turn_failure(e: &aura_agent::AgentError, cancelled: bool, limit: Option<Duration>) -> String {
    match (e, limit) {
        _ if cancelled => "cancelled by the user".to_string(),
        (aura_agent::AgentError::Timeout(_), Some(limit)) => format!(
            "ran out of time: it stopped after {} minutes without a reply",
            limit.as_secs() / 60
        ),
        _ => e.to_string(),
    }
}

/// How a subtask turn begun at message `turn_start`, with time `limit`,
/// is closed.
struct TurnEnding<'a> {
    ui: &'a AgentUi,
    cancel: &'a CancellationToken,
    turn_start: usize,
    limit: Option<Duration>,
}

impl TurnEnding<'_> {
    /// How the turn that ran as `result` ended, its prompt `recorded`.
    async fn conclude(
        &self,
        session: &mut AgentSession,
        result: Result<aura_agent::AgentLoopResult, aura_agent::AgentError>,
        forwarded: &ForwarderState,
        recorded: Option<(Hash, PromptCategory)>,
    ) -> TaskEnd {
        let cancelled = self.cancel.is_cancelled();
        let signal = if cancelled || forwarded.credentials_rejected {
            Some(OutcomeSignal::Cancelled)
        } else {
            ended_signal(&result, self.turn_start)
        };
        let mut end = TaskEnd {
            reply: Ok(String::new()),
            prompt: recorded,
            signal,
            cost_usd: forwarded.usage.cost_usd,
            files: Vec::new(),
            shown: (String::new(), String::new()),
        };
        let result = match result {
            Ok(result) => result,
            Err(e) => {
                let reason = turn_failure(&e, cancelled, self.limit);
                self.ui.send(UiCommand::ShowWarning(reason.clone())).await;
                self.ui.send(UiCommand::Complete).await;
                end.reply = Err(reason);
                return end;
            }
        };
        self.settle(session, result, forwarded, end).await
    }

    /// Fill in `end` from the turn's `result` and hand the result to
    /// `session`.
    async fn settle(
        &self,
        session: &mut AgentSession,
        result: aura_agent::AgentLoopResult,
        forwarded: &ForwarderState,
        mut end: TaskEnd,
    ) -> TaskEnd {
        let text = result.final_text.clone();
        let llm_error = result.llm_error.as_ref().map(ToString::to_string);
        end.shown = shown_to_judge(result.messages.get(self.turn_start..).unwrap_or_default());
        end.files = result
            .file_changes
            .iter()
            .map(|change| change.path.clone())
            .collect();
        handle_agent_success(session, result, forwarded, self.ui, &[]).await;
        end.reply = match llm_error {
            _ if self.cancel.is_cancelled() => Err("cancelled by the user".to_string()),
            Some(err) if text.trim().is_empty() => Err(err),
            _ => Ok(text),
        };
        end
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
