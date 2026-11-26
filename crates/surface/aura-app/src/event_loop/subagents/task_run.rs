use super::*;

impl AgentHub {
    /// Route `run`'s subtask, run its turn to the end, run its check and
    /// record how it went, then hand its session back for the event loop
    /// to keep.
    pub(super) async fn finish_task(&self, run: TaskRun) -> TaskDone {
        let ended = self.run_attempt(run).await;
        self.settle_attempt(ended).await
    }

    /// Route `run`'s subtask, run its turn to the end and run its check.
    async fn run_attempt(&self, mut run: TaskRun) -> Ended {
        let limits = self.turn_limits(run.deadline);
        let child_id = run.session.agent_id();
        let ui = AgentUi::new(self.commands.clone(), child_id);
        let started = Instant::now();
        let label = run
            .gate
            .label(&run.session, &mut run.config, &run.prompt, &run.cancel)
            .await;
        run.summary.model = Some(run.config.model.clone());
        run.trace.model.clone_from(&run.config.model);
        let recorder = self.traces.start(child_id, run.trace);
        let workspace = run.session.kernel.agent_workspace();
        self.register_root(child_id, &run.name, workspace, &run.config);
        self.processes.start(run.summary, Some(run.cancel.clone()));
        let files = self.attempt_files(&run.gate, child_id);
        let agent_loop = AgentLoop::new(run.config);
        let turn = TaskTurn {
            agent_loop: &agent_loop,
            tools: run.tools,
            prompt: &run.prompt,
            label,
            ui: &ui,
            cancel: &run.cancel,
            limits,
            recorder,
        };
        let mut end = run_task_turn(&mut run.session, turn).await;
        let cancelled = run.cancel.is_cancelled();
        let check = run_task_check(&run.session, &ui, run.check, &mut end, cancelled).await;
        let files_before = files.map(|files| files.take_attempt(child_id));
        Ended {
            session: run.session,
            agent_loop,
            ui,
            prompt: run.prompt,
            gate: run.gate,
            end,
            check,
            files_before,
            cancelled,
            started,
            parent: run.parent,
            name: run.name,
            running: run.running,
        }
    }

    /// The hub's limits for a turn, with its time cut to `deadline`.
    fn turn_limits(&self, deadline: Option<Instant>) -> LongRunSettings {
        let mut limits = self.long_run;
        if let Some(deadline) = deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            limits.turn_timeout = Some(limits.turn_timeout.map_or(left, |limit| limit.min(left)));
        }
        limits
    }

    /// The checkpoints of the files `child` changes, kept when `gate`
    /// may retry its first attempt.
    fn attempt_files(
        &self,
        gate: &SubtaskGate,
        child: AgentId,
    ) -> Option<crate::file_checkpoints::FileCheckpoints> {
        let files = self
            .factory
            .file_checkpoints
            .clone()
            .filter(|_| gate.candidates.is_some() && gate.escalation.is_none());
        if let Some(files) = &files {
            files.begin_attempt(child);
        }
        files
    }

    /// Close `ended`'s Processes row, have Gate record and judge how it
    /// went, then hand its session back for the event loop to keep.
    async fn settle_attempt(&self, ended: Ended) -> TaskDone {
        let child_id = ended.session.agent_id();
        let failure = ended.failure();
        self.processes.finish(
            &task_id(child_id),
            if ended.end.reply.is_ok() {
                ProcessStatus::Exited
            } else {
                ProcessStatus::Failed
            },
        );
        let model = ended.agent_loop.config().model.clone();
        let effort = routed_effort(ended.agent_loop.config());
        let category = ended.end.prompt.map(|(_, category)| category);
        let Judgement {
            outcome,
            judged,
            unfinished,
        } = ended.judge(&model, failure.as_deref()).await;
        if let Some(redo) = self.end_attempt(ended.parent, child_id, outcome) {
            redo.record().await;
        }

        let Ended {
            session,
            end,
            check,
            files_before,
            cancelled,
            name,
            running,
            ..
        } = ended;
        // `park` ends the running from here, and nothing can return or be
        // dropped between the two.
        running.disarm();
        self.park(session);
        self.send_agent_list().await;
        TaskDone {
            child: child_id,
            name,
            model,
            effort,
            reply: end.reply,
            category,
            files: end.files,
            check,
            files_before,
            cancelled,
            judged,
            unfinished,
        }
    }
}

/// Run `check` in `session`'s workspace when the turn that ended as `end`
/// replied and was not `cancelled`, showing how it went in `ui` and
/// noting what it says on `end`.
async fn run_task_check(
    session: &AgentSession,
    ui: &AgentUi,
    check: Option<String>,
    end: &mut TaskEnd,
    cancelled: bool,
) -> Option<task_check::CheckRun> {
    match check {
        Some(command) if end.reply.is_ok() && !cancelled => {
            let run = task_check::run_check(session, &command).await;
            ui.send(system_message(run.describe())).await;
            if let Some(signal) = run.signal() {
                end.signal = Some(signal);
            }
            Some(run)
        }
        _ => None,
    }
}

/// A subtask attempt whose turn and check are over.
struct Ended {
    session: AgentSession,
    agent_loop: AgentLoop,
    ui: AgentUi,
    prompt: String,
    gate: SubtaskGate,
    end: TaskEnd,
    check: Option<task_check::CheckRun>,
    files_before: Option<AttemptFiles>,
    cancelled: bool,
    /// When the attempt started
    started: Instant,
    /// The agent that dispatched it
    parent: AgentId,
    /// The instance's name
    name: String,
    running: RunningGuard,
}

impl Ended {
    /// Why the attempt failed, unless it was cancelled.
    fn failure(&self) -> Option<String> {
        task_check::Attempted {
            model: &self.agent_loop.config().model,
            reply: &self.end.reply,
            files: &self.end.files,
            check: self.check.as_ref(),
        }
        .failure()
        .filter(|_| !self.cancelled)
    }

    /// Have Gate record how the attempt on `model` went and judge it,
    /// given its `failure`.
    async fn judge(&self, model: &str, failure: Option<&str>) -> Judgement {
        let end = &self.end;
        self.gate
            .finish(SubtaskEnd {
                session: &self.session,
                ui: &self.ui,
                task: &self.prompt,
                model,
                auth_token: self.agent_loop.config().auth_token.as_deref(),
                prompt: end.prompt,
                signal: end.signal,
                cost_usd: end.cost_usd,
                started: self.started,
                reply: end.reply.as_deref().unwrap_or_default(),
                files_changed: end.files.len(),
                shown: &end.shown,
                files_before: self.files_before.as_ref(),
                failure,
            })
            .await
    }
}

/// A spawned permanent agent's turn, ready to run.
pub(super) struct TaskRun {
    pub(super) session: AgentSession,
    /// Its agent loop's config, before Gate routes it
    pub(super) config: AgentLoopConfig,
    pub(super) tools: Vec<ToolDefinition>,
    pub(super) prompt: String,
    pub(super) cancel: CancellationToken,
    pub(super) gate: SubtaskGate,
    /// Its Processes tab row
    pub(super) summary: ProcessSummary,
    /// The instance's name
    pub(super) name: String,
    /// The agent that dispatched it
    pub(super) parent: AgentId,
    /// Command run after its turn, whose exit status is its verdict
    pub(super) check: Option<String>,
    /// Its run trace, started once Gate has picked its model
    pub(super) trace: RunMeta,
    /// Ends its running if the attempt is dropped before `park`
    pub(super) running: RunningGuard,
    /// When it must stop because its caller stops waiting for it
    pub(super) deadline: Option<Instant>,
}

/// A `task` call being worked on.
pub(super) struct Subtask {
    pub(super) request: SubagentDispatchRequest,
    pub(super) definition: AgentDefinition,
    pub(super) first: TaskRun,
}

/// How one attempt at a subtask ended.
pub(super) struct TaskDone {
    pub(super) child: AgentId,
    /// The instance's name
    pub(super) name: String,
    /// Model it ran on
    pub(super) model: String,
    /// The effort Gate chose for it, while that was Gate's to choose
    pub(super) effort: Option<ThinkingEffort>,
    /// Its final reply, or why there is none
    pub(super) reply: Result<String, String>,
    /// Its prompt's category, when the prompt was recorded
    pub(super) category: Option<PromptCategory>,
    /// Files its file tools changed
    pub(super) files: Vec<String>,
    /// How its check went, when it had one and got that far
    pub(super) check: Option<task_check::CheckRun>,
    /// What the files it changed held before, when it may be retried
    pub(super) files_before: Option<AttemptFiles>,
    pub(super) cancelled: bool,
    /// What Gate's judge found, when the attempt failed and was reviewed
    pub(super) judged: Option<Verdict>,
    /// Why the attempt failed when its check and reply did not say so:
    /// the judge found its unchecked work unfinished
    pub(super) unfinished: Option<String>,
}

impl TaskDone {
    pub(super) fn attempted(&self) -> task_check::Attempted<'_> {
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
pub(super) struct TaskReport {
    pub(super) first: TaskDone,
    pub(super) retry: Option<(TaskDone, String)>,
}

impl TaskReport {
    /// The attempt whose result the caller gets.
    pub(super) fn last(&self) -> &TaskDone {
        self.retry.as_ref().map_or(&self.first, |(retry, _)| retry)
    }

    /// The compact result the caller gets.
    pub(super) fn text(&self) -> String {
        let retried = self
            .retry
            .as_ref()
            .map(|(_, why)| (self.first.model.as_str(), why.as_str()));
        task_check::compact(&self.last().attempted(), retried)
    }

    /// The reply, or why there is none, for a background task's notice.
    pub(super) fn outcome(&self) -> Result<String, String> {
        match &self.last().reply {
            Ok(_) => Ok(self.text()),
            Err(reason) => Err(reason.clone()),
        }
    }

    /// The result of the `task` call.
    pub(super) fn result(self) -> SubagentResult {
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
pub(super) struct WriteSlot<'a> {
    pub(super) hub: &'a AgentHub,
    /// The workspace's lock; `None` for a subtask of one that holds it,
    /// or when the wait was cancelled
    pub(super) guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    /// Agents working in the slot, whose own write subtasks need not wait
    pub(super) holders: Vec<AgentId>,
}

impl WriteSlot<'_> {
    pub(super) fn hold(&mut self, agent: AgentId) {
        lock(&self.hub.writing).insert(agent);
        self.holders.push(agent);
    }

    /// Whether this slot took the workspace's lock itself.
    pub(super) fn exclusive(&self) -> bool {
        self.guard.is_some()
    }
}
