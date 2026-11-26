//! Building an agent's kernel, and the check notices it sends.

use super::*;

/// Constructs the kernel for one agent, given the agents that spawned it
/// (nearest first) and its project's folder (`None` uses the launch
/// directory).
pub type KernelBuilder =
    Box<dyn Fn(AgentId, Vec<AgentId>, Option<PathBuf>) -> anyhow::Result<Kernel> + Send + Sync>;

/// Opens per-agent sessions and reads agent records for the UI.
pub struct KernelFactory {
    pub(crate) store: Arc<dyn ReadStore>,
    pub(super) build_kernel: KernelBuilder,
    pub(crate) checkpoints: Option<Checkpoints>,
    pub(crate) file_checkpoints: Option<FileCheckpoints>,
    pub(super) post_edit_check: Option<(PostEditCheckConfig, mpsc::Sender<UiCommand>)>,
    /// What the kernels' record observer is; entries copied from cloud
    /// machines go to it too
    pub(crate) record_observer: Option<Arc<dyn RecordObserver>>,
    /// Every agent's current task; the hub gives it a way to show them
    pub(crate) tasks: Arc<TaskBoard>,
    /// Prompt-cache scope of each agent whose kernel is scoped: a
    /// `/compare` run's agents, and the agents opened under them
    pub(super) cache_salts: Mutex<HashMap<AgentId, String>>,
    /// Scratch folder of each agent whose commands get their own: a
    /// `/compare` run's agents, and the agents opened under them
    pub(super) run_scratches: Mutex<HashMap<AgentId, RunScratch>>,
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
            record_observer: None,
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
pub(super) fn inherited<T: Clone>(
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

    /// Note `observer`, the kernels' record observer, for the entries
    /// copied from cloud machines.
    #[must_use]
    pub fn with_record_observer(mut self, observer: Option<Arc<dyn RecordObserver>>) -> Self {
        self.record_observer = observer;
        self
    }

    /// The agent's current checkpoint with what the record holds beyond
    /// it (in `tail`, its newest entries), when the checkpoint still
    /// applies.
    fn restore(&self, agent_id: AgentId, tail: &RecordTail) -> Option<SessionCheckpoint> {
        aura_agent::history::restore_with(self.checkpoints.as_ref()?, agent_id, |after_seq| {
            tail.model_history_since(after_seq)
        })
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
        let tail = read_tail(self.store.as_ref(), agent.agent_id());
        self.open_from(agent, parent_chain, folder, &tail)
    }

    /// [`Self::open`] with `tail`, the newest entries of the agent's
    /// record, read already.
    ///
    /// # Errors
    /// Returns an error if the kernel cannot be constructed.
    pub fn open_from(
        &self,
        agent: &RegisteredAgent,
        parent_chain: Vec<AgentId>,
        folder: Option<PathBuf>,
        tail: &RecordTail,
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
        // A finished plan is not picked back up; the next `/plan` writes
        // a new one.
        let active_plan = latest_plan_path_in(tail.entries())
            .filter(|path| !plan_mode::is_finished_plan(&kernel.agent_workspace(), path));
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
        if let Some(checkpoint) = self.restore(session.agent_id(), tail) {
            info!(agent = %session.agent_id(), "Resuming session from checkpoint");
            session.restore(checkpoint);
        } else {
            let messages = model_history_from(tail.entries());
            session.todos = plan_mode::unfinished_todos(todos_from_history(&messages));
            session.messages = messages;
            session.resume_plan_build();
        }
        Ok(session)
    }
}

/// The newest entries of `agent_id`'s record, or none when they can't be
/// read: the agent then opens with an empty conversation.
pub(super) fn read_tail(store: &dyn ReadStore, agent_id: AgentId) -> RecordTail {
    RecordTail::read(store, agent_id).unwrap_or_else(|e| {
        warn!(%agent_id, error = %e, "Failed to read the record");
        RecordTail::default()
    })
}

/// Shows post-edit check results in the agent's chat. The model gets the
/// failure output itself; the chat only says what happened.
pub(super) fn check_notifier(
    commands: mpsc::Sender<UiCommand>,
    agent_id: AgentId,
) -> CheckNotifier {
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
pub(crate) fn check_notice(report: &CheckReport, first: bool) -> Option<UiCommand> {
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
