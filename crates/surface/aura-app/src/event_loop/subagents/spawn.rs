use super::*;
use crate::agent_registry::RegisteredAgent;

impl AgentHub {
    pub(super) async fn spawn(
        self: &Arc<Self>,
        request: SubagentDispatchRequest,
    ) -> Result<SubagentResult, String> {
        let definition = match self.spawnable(&request.subagent_type) {
            Ok(definition) => definition,
            Err(problem) => return Ok(SubagentResult::rejected(problem)),
        };
        let detached = request.spawn_mode == Some(SpawnMode::Detached);
        let started = self.start_subtask(request, definition).await?;
        if detached {
            return Ok(self.run_detached(started));
        }
        Ok(self.run_waited(started).await)
    }

    /// The permanent agent named `name` that a `task` call may spawn, or
    /// why the call is rejected, listing those it may.
    fn spawnable(&self, name: &str) -> Result<AgentDefinition, String> {
        let definition = lock(&self.definitions)
            .get(name)
            .filter(|d| !d.is_coordinator())
            .cloned();
        definition.ok_or_else(|| {
            let available: Vec<String> = lock(&self.definitions)
                .all()
                .iter()
                .filter(|d| !d.is_coordinator())
                .map(|d| d.name.clone())
                .collect();
            format!(
                "task: no permanent agent named `{}`; available: {}",
                name,
                available.join(", ")
            )
        })
    }

    /// Open the first attempt of `request` on an instance of `definition`,
    /// record and list the spawn and post its request to the INBOX.
    async fn start_subtask(
        self: &Arc<Self>,
        request: SubagentDispatchRequest,
        definition: AgentDefinition,
    ) -> Result<StartedSubtask, String> {
        let first = self
            .open_task(&request, &definition, request.prompt.clone(), None)
            .await?;
        let parent = request.parent_agent_id;
        let child_id = first.session.agent_id();
        self.begin_first_attempt(parent, &definition.name, &request.prompt, &first)
            .await;
        let name = first.name.clone();
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
        let mail = self.task_mail(parent, child_id, &name, &request.prompt);
        self.post_mail(mail.request(&request.prompt));
        let todo = request.todo_id.clone();
        Ok(StartedSubtask {
            subtask: Subtask {
                request,
                definition,
                first,
            },
            mail,
            parent,
            child_id,
            name,
            todo,
        })
    }

    /// Note `first`, the first attempt at a subtask of kind `kind` asked
    /// `prompt`, among `parent`'s attempts and record those it redoes.
    async fn begin_first_attempt(
        &self,
        parent: AgentId,
        kind: &str,
        prompt: &str,
        first: &TaskRun,
    ) {
        let redos = self.begin_attempt(
            parent,
            Attempt {
                kind: kind.to_string(),
                prompt: prompt.to_string(),
                child: first.session.agent_id(),
                kernel: Arc::clone(&first.session.kernel),
                ended: None,
                redone: false,
            },
        );
        for redo in redos {
            redo.record().await;
        }
    }

    /// Run `started` in the background, its reply arriving as a notice,
    /// and tell the caller so.
    fn run_detached(self: &Arc<Self>, started: StartedSubtask) -> SubagentResult {
        let StartedSubtask {
            subtask,
            mail,
            parent,
            child_id,
            name,
            todo,
        } = started;
        info!(child = %name, "Running permanent agent in the background");
        let hub = Arc::clone(self);
        let shown = name.clone();
        tokio::spawn(async move {
            let report = hub.run_subtask(subtask).await;
            hub.factory
                .tasks
                .finish_spawn(child_id, report.outcome())
                .await;
            let last = report.last();
            hub.post_mail(mail.reply(last.child, &last.name, report.outcome()));
            let _ = hub.notices.send(task_notice(
                parent,
                &shown,
                todo.as_deref(),
                report.outcome(),
            ));
        });
        SubagentResult::completed(
            child_id,
            format!(
                "{name} is working on this in the background. Its reply will arrive \
                 as a message when it finishes; carry on meanwhile."
            ),
        )
    }

    /// Run `started` to its end and return its result to the caller.
    async fn run_waited(self: &Arc<Self>, started: StartedSubtask) -> SubagentResult {
        let StartedSubtask {
            subtask,
            mail,
            child_id,
            ..
        } = started;
        let report = self.run_subtask(subtask).await;
        self.factory
            .tasks
            .finish_spawn(child_id, report.outcome())
            .await;
        let last = report.last();
        self.post_mail(mail.reply(last.child, &last.name, report.outcome()));
        if let Err(reason) = &last.reply {
            warn!(child = %last.name, %reason, "Permanent agent failed");
        }
        report.result()
    }

    /// The INBOX thread of the `task` `parent` gave `child`, shown as
    /// `child_name`, named after `prompt`.
    pub(super) fn task_mail(
        &self,
        parent: AgentId,
        child: AgentId,
        child_name: &str,
        prompt: &str,
    ) -> TaskMail {
        let parent_name = lock(&self.registry)
            .find(parent)
            .map_or_else(|| "agent".to_string(), |a| a.display_name().into_owned());
        TaskMail::new(parent, parent_name, child, child_name.to_string(), prompt)
    }

    /// Write on the parent's record that `request` went to `child`, an
    /// instance of `definition` shown as `name`: which `task` call it
    /// answers, and the todo it does. A failure is logged; the child's
    /// own record still says who spawned it.
    pub(super) async fn record_spawn(
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
    pub(super) async fn open_task(
        self: &Arc<Self>,
        request: &SubagentDispatchRequest,
        definition: &AgentDefinition,
        prompt: String,
        escalation: Option<(Decision, PromptCategory)>,
    ) -> Result<TaskRun, String> {
        let (instance, session) = self.open_instance(request, definition)?;
        let child_id = session.agent_id();
        let cancel = CancellationToken::new();
        // From here on the guard ends the running if this attempt never
        // reaches `park`: an early return, or the future being dropped.
        let running = self.start_guarded(child_id, Arc::clone(&session.kernel), cancel.clone());
        self.send_agent_list().await;

        let workspace = session.kernel.agent_workspace();
        let first = escalation.is_none();
        let (mut config, tools) = self
            .subtask_turn(request, definition, &session, &prompt, first)
            .await;
        let mut trace = RunMeta::new(&config.model, RunSource::Tui);
        trace.workspace = workspace.display().to_string();
        trace.mode.clone_from(&definition.name);
        trace.spawned_by = self.traces.run_id_of(request.parent_agent_id);
        let mut gate = self.subtask_gate(request, definition, &mut config);
        gate.escalation = escalation;
        if reads_only(definition)
            && config.user_thinking_effort.is_none()
            && self.in_bench(request.parent_agent_id)
        {
            config.user_thinking_effort = Some(ThinkingEffort::Medium);
        }
        let deadline = (request.spawn_mode != Some(SpawnMode::Detached))
            .then(|| Instant::now() + config.per_tool_timeout.saturating_sub(STOP_BEFORE_CALLER));
        let summary = spawn_summary(
            request,
            &prompt,
            &instance.name,
            definition,
            child_id,
            &config,
        );
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
            running,
            deadline,
        })
    }

    /// Register an instance of `definition` under `request`'s caller and
    /// open its session, in its caller's mode.
    fn open_instance(
        &self,
        request: &SubagentDispatchRequest,
        definition: &AgentDefinition,
    ) -> Result<(RegisteredAgent, AgentSession), String> {
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
        crate::event_loop::channels::join_or_warn(self, &instance);
        let session = self
            .factory
            .open(&instance, request.parent_chain.clone(), folder)
            .map_err(|e| format!("opening {}: {e:#}", instance.name))?;
        let child_id = session.agent_id();
        info!(child = %instance.name, definition = %definition.name, "Spawning permanent agent");
        // A spawn asks no less than the agent that started it.
        let permissions = self.permissions_of(request.parent_agent_id);
        self.set_permissions(child_id, permissions);
        crate::event_loop::modes::apply(&session.kernel, permissions, &self.tool_names());
        Ok((instance, session))
    }

    /// The turn config and tools of `session`, an instance of `definition`
    /// asked `prompt` for `request`, named as it starts; only the `first`
    /// attempt opens the task's thread.
    async fn subtask_turn(
        self: &Arc<Self>,
        request: &SubagentDispatchRequest,
        definition: &AgentDefinition,
        session: &AgentSession,
        prompt: &str,
        first: bool,
    ) -> (AgentLoopConfig, Vec<ToolDefinition>) {
        let child_id = session.agent_id();
        let workspace = session.kernel.agent_workspace();
        self.send_instructions(child_id, &workspace).await;
        let (mut config, tools) = self.agent_turn(child_id, Some(&definition.name), &workspace);
        config.initial_todos.clone_from(&session.todos);
        // Observers report on the agent they were installed for, not this one.
        config.observers.clear();
        // Named beside the turn, not after it, so the name shows within a
        // second or two. Only the first attempt opens the task's thread
        // (`task:<first child>`); a retry posts to it.
        let thread = first.then(|| task_id(child_id));
        self.name_subtask(request, child_id, &session.kernel, &config, prompt, thread)
            .await;
        if let Some(addendum) = request
            .system_prompt_addendum
            .as_deref()
            .filter(|a| !a.trim().is_empty())
        {
            config.system_prompt.push_str("\n\n");
            config.system_prompt.push_str(addendum);
        }
        (config, tools)
    }

    /// Whether `agent` works under a benchmark: it, or an agent above it
    /// in the registry, runs a benchmark's run (its trace is a bench
    /// run), or it is a `/compare` run or a subtask of one. So a subtask
    /// nested under a run (bench arm, `REN1`, `REV1`) is treated like the
    /// run's direct subtasks, though its own trace is a TUI run.
    /// [`AgentRegistry::ancestry`] stops after as many steps as there are
    /// agents, so a hand-edited parent cycle cannot loop.
    pub(super) fn in_bench(&self, agent: AgentId) -> bool {
        let (mut ancestry, under_compare) = {
            let registry = lock(&self.registry);
            (registry.ancestry(agent), registry.under_compare(agent))
        };
        if ancestry.is_empty() {
            // Unregistered: only its own trace can say.
            ancestry.push(agent);
        }
        under_compare
            || ancestry.into_iter().any(|id| {
                self.traces
                    .recorder(id)
                    .is_some_and(|rec| rec.meta().source == RunSource::Bench)
            })
    }
}

/// A `task` call whose first attempt is open and listed.
struct StartedSubtask {
    subtask: Subtask,
    /// Its INBOX thread
    mail: TaskMail,
    parent: AgentId,
    child_id: AgentId,
    /// The instance's name
    name: String,
    /// The todo it does
    todo: Option<String>,
}

/// The Processes tab row of the spawn of `definition` running as
/// `child` on `config`, asked `prompt` for `request`.
fn spawn_summary(
    request: &SubagentDispatchRequest,
    prompt: &str,
    name: &str,
    definition: &AgentDefinition,
    child: AgentId,
    config: &AgentLoopConfig,
) -> ProcessSummary {
    let mut summary = task_summary(
        request,
        prompt,
        name,
        &definition.name,
        child,
        &config.model,
    );
    if request.spawn_mode == Some(SpawnMode::Detached) {
        summary.kind = ProcessKind::Agent;
    }
    summary
}
