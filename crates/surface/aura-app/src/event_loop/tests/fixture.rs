//! The event loop test fixture and the helpers that drive its loop.

use super::*;

pub(super) struct Fixture {
    pub(super) hub: Arc<AgentHub>,
    /// Runs the `task` calls of the hub's kernels
    pub(super) dispatcher: Arc<dyn aura_exec_tools::SubagentDispatchHook>,
    pub(super) config: AgentLoopConfig,
    pub(super) gate: Arc<Semaphore>,
    pub(super) commands_tx: mpsc::Sender<UiCommand>,
    pub(super) approvals: Arc<TerminalApprovals>,
    pub(super) first: AgentId,
    pub(super) second: AgentId,
    pub(super) files: crate::file_checkpoints::FileCheckpoints,
    pub(super) workspace: std::path::PathBuf,
    /// Every request the hub's kernels sent
    pub(super) sent: SentLog,
    pub(super) _dirs: Vec<TempDir>,
}

impl Fixture {
    /// The requests `agent`'s kernels sent, oldest first.
    pub(super) fn sent_by(&self, agent: AgentId) -> Vec<Sent> {
        (self.sent.lock().unwrap().iter())
            .filter(|sent| sent.agent == agent)
            .cloned()
            .collect()
    }
}

/// Gate settings for tests: the defaults, without exploring.
pub(super) fn steady_gate() -> GateLive {
    GateLive::new(aura_config::GateConfig::defaults().without_exploring())
}

/// [`steady_gate`] whose lead keeps its own file tools and commands, as
/// with `AURA_GATE_DELEGATE=off`.
pub(super) fn steady_gate_without_delegation() -> GateLive {
    GateLive::new(aura_config::GateConfig {
        delegate: false,
        ..aura_config::GateConfig::defaults().without_exploring()
    })
}

pub(super) fn fixture() -> (Fixture, mpsc::Receiver<UiCommand>) {
    fixture_with_gate(steady_gate())
}

/// [`fixture`] whose kernels feed `gate`.
pub(super) fn fixture_with_gate(gate: GateLive) -> (Fixture, mpsc::Receiver<UiCommand>) {
    let env = Env::new(gate);
    let (commands_tx, commands) = mpsc::channel(4096);
    let mut registry =
        AgentRegistry::load_or_init_in(env.data.path(), env.workspace.path()).unwrap();
    let first = registry.active().agent_id();
    let second = registry
        .create(Some("Second".to_string()))
        .unwrap()
        .agent_id();
    let approvals = Arc::new(TerminalApprovals::new(commands_tx.clone(), env.data.path()));
    let parts = env.hub_parts(registry, &commands_tx, &approvals);
    let kernels = KernelParts::new(&env, &commands_tx, &approvals);
    let gate = Arc::clone(&kernels.provider_gate);
    let sent = Arc::clone(&kernels.sent);
    let files = crate::file_checkpoints::FileCheckpoints::new(env.data.path());
    let mut captured = None;
    let hub = AgentHub::new(parts, |dispatcher| {
        captured = Some(dispatcher);
        kernels.factory(&env, files.clone(), commands_tx.clone())
    });
    let Env {
        config,
        data,
        db,
        workspace,
        ..
    } = env;
    let fixture = Fixture {
        hub,
        dispatcher: captured.unwrap(),
        config,
        gate,
        commands_tx,
        approvals,
        first,
        second,
        files,
        workspace: workspace.path().canonicalize().unwrap(),
        sent,
        _dirs: vec![data, db, workspace],
    };
    (fixture, commands)
}

/// The folders, store, config and Gate a fixture is built on.
struct Env {
    gate: Arc<GateLive>,
    data: TempDir,
    db: TempDir,
    workspace: TempDir,
    store: Arc<RocksStore>,
    config: AgentLoopConfig,
}

impl Env {
    fn new(gate: GateLive) -> Self {
        let db = TempDir::new().unwrap();
        Self {
            gate: Arc::new(gate),
            data: TempDir::new().unwrap(),
            store: Arc::new(RocksStore::open(db.path(), false).unwrap()),
            db,
            workspace: TempDir::new().unwrap(),
            config: default_agent_config_with_auth("mock-model", None),
        }
    }

    /// The parts of the fixture's hub.
    fn hub_parts(
        &self,
        registry: AgentRegistry,
        commands_tx: &mpsc::Sender<UiCommand>,
        approvals: &Arc<TerminalApprovals>,
    ) -> AgentHubParts {
        let (_, tools) = build_executor_router_with_config(&ToolConfig::for_autonomous_dev_loop());
        let traces = Traces::new(
            Some(self.data.path().join("home")),
            Some(Arc::new(aura_store_db::TraceStore::for_store(&self.store))),
            None,
            commands_tx.clone(),
        );
        AgentHubParts {
            registry,
            definitions: AgentDefinitions::load_or_seed(self.data.path()).unwrap(),
            skills: SkillManager::new(SkillLoader::with_defaults(
                Some(self.workspace.path().to_path_buf()),
                None,
            )),
            base: TurnBase {
                limits: crate::model_selection::ModelLimits::of(&self.config),
                config: self.config.clone(),
                tools,
                gate: GateBase::default(),
            },
            commands: commands_tx.clone(),
            long_run: crate::long_run::LongRunSettings::default(),
            mode: PermissionMode::Agent,
            approval: aura_ui::ApprovalLevel::Ask,
            approvals: Some(Arc::clone(approvals)),
            instructions: crate::project_instructions::ProjectInstructions::only_in(
                self.workspace.path(),
            ),
            processes: ProcessTable::without_os_scan(None, None),
            agents: None,
            record_panel: RecordPanelAgent::default(),
            gate: Arc::clone(&self.gate),
            traces,
            channels: Some(aura_store_db::ChannelStore::for_store(&self.store)),
        }
    }
}

/// What the fixture's kernel builder shares between the kernels it builds.
#[derive(Clone)]
struct KernelParts {
    provider_gate: Arc<Semaphore>,
    judges: Arc<Barrier>,
    sent: SentLog,
    workspace_root: std::path::PathBuf,
    prompter: Arc<TerminalApprovals>,
    record_observer: Arc<dyn aura_agent_kernel::RecordObserver>,
}

impl KernelParts {
    fn new(
        env: &Env,
        commands_tx: &mpsc::Sender<UiCommand>,
        approvals: &Arc<TerminalApprovals>,
    ) -> Self {
        let record_observer: Arc<dyn aura_agent_kernel::RecordObserver> =
            Arc::new(FanoutObserver(vec![
                Arc::new(UiRecordObserver::new(
                    commands_tx.clone(),
                    RecordPanelAgent::default(),
                )) as Arc<dyn aura_agent_kernel::RecordObserver>,
                Arc::clone(&env.gate) as Arc<dyn aura_agent_kernel::RecordObserver>,
            ]));
        Self {
            provider_gate: Arc::new(Semaphore::new(0)),
            judges: Arc::new(Barrier::new(2)),
            sent: SentLog::default(),
            workspace_root: env.workspace.path().to_path_buf(),
            prompter: Arc::clone(approvals),
            record_observer,
        }
    }

    /// The fixture's kernel factory, whose kernels record file changes in
    /// `files`.
    fn factory(
        &self,
        env: &Env,
        files: crate::file_checkpoints::FileCheckpoints,
        check_commands: mpsc::Sender<UiCommand>,
    ) -> KernelFactory {
        let parts = self.clone();
        let store = Arc::clone(&env.store);
        let hook_files = files.clone();
        KernelFactory::new(
            Arc::clone(&env.store) as Arc<dyn aura_store_db::ReadStore>,
            Box::new(move |agent_id, parent_chain, folder| {
                parts.kernel(&store, &hook_files, agent_id, parent_chain, folder)
            }),
        )
        .with_checkpoints(crate::session_checkpoint::Checkpoints::new(env.data.path()))
        .with_file_checkpoints(files)
        // Switched off, so no session runs it, but every session has one
        // to look at.
        .with_post_edit_check(
            aura_agent::PostEditCheckConfig::from_lookup(|name| {
                (name == aura_agent::post_edit_check::CHECK_COMMAND_ENV).then(|| "off".to_string())
            }),
            check_commands,
        )
    }

    /// The kernel of `agent_id`, spawned by `parent_chain`, in `folder`.
    fn kernel(
        &self,
        store: &Arc<RocksStore>,
        hook_files: &crate::file_checkpoints::FileCheckpoints,
        agent_id: AgentId,
        parent_chain: Vec<AgentId>,
        folder: Option<std::path::PathBuf>,
    ) -> anyhow::Result<Kernel> {
        let provider: Arc<dyn ModelProvider + Send + Sync> = Arc::new(GatedProvider {
            inner: MockProvider::simple_response("Done."),
            gate: Arc::clone(&self.provider_gate),
            judges: Arc::clone(&self.judges),
            agent: agent_id,
            sent: Arc::clone(&self.sent),
        });
        let workspace_base = folder.unwrap_or_else(|| self.workspace_root.clone());
        self.prompter.set_folder(agent_id, workspace_base.clone());
        let chain = std::iter::once(agent_id).chain(parent_chain).collect();
        let executor = aura_exec_tools::ToolExecutor::new(ToolConfig::for_autonomous_dev_loop())
            .with_file_change_hook(hook_files.hook(chain, None));
        let mut executors = aura_agent_kernel::ExecutorRouter::new();
        executors.add_executor(Arc::new(executor));
        Ok(Kernel::new(
            Arc::clone(store) as Arc<dyn aura_store_db::Store>,
            provider,
            executors,
            KernelConfig {
                workspace_base,
                use_workspace_base_as_root: true,
                tool_approval_prompter: Some(
                    Arc::clone(&self.prompter) as Arc<dyn aura_agent_kernel::ToolApprovalPrompter>
                ),
                record_observer: Some(Arc::clone(&self.record_observer)),
                ..KernelConfig::default()
            },
            agent_id,
        )?)
    }
}

/// Everything a [`LoopState`] borrows.
pub(super) struct Borrowed {
    pub(super) events: mpsc::Receiver<UiEvent>,
    pub(super) agent_loop: AgentLoop,
    pub(super) done: mpsc::Receiver<WorkDone>,
    pub(super) done_tx: mpsc::Sender<WorkDone>,
    pub(super) links: mpsc::Sender<auth::LinkOutcome>,
    pub(super) _auth_dir: TempDir,
    pub(super) auth: Arc<AuthManager>,
}

pub(super) fn borrowed(fixture: &Fixture) -> Borrowed {
    let (_, events) = mpsc::channel(1);
    let (done_tx, done) = mpsc::channel(64);
    let (links, _) = mpsc::channel(1);
    let auth_dir = TempDir::new().unwrap();
    let auth = Arc::new(AuthManager::with_identity(
        Vault::files_at(auth_dir.path()),
        Arc::new(ZosIdentity::new().unwrap()),
    ));
    Borrowed {
        events,
        agent_loop: AgentLoop::new(fixture.config.clone()),
        done,
        done_tx,
        links,
        _auth_dir: auth_dir,
        auth,
    }
}

pub(super) fn loop_state<'a>(fixture: &'a Fixture, b: &'a mut Borrowed) -> LoopState<'a> {
    let first = lock_registry_agent(fixture, fixture.first);
    let session = fixture.hub.factory.open(&first, Vec::new(), None).unwrap();
    LoopState {
        commands: &fixture.commands_tx,
        events: &mut b.events,
        agent_loop: &mut b.agent_loop,
        memory_manager: None,
        models: ModelSelection::fixed(&fixture.config, "mock-model"),
        active: fixture.first,
        idle: HashMap::from([(fixture.first, session)]),
        queued: queue::WorkQueues::default(),
        work_done: b.done_tx.clone(),
        in_flight: 0,
        hub: Arc::clone(&fixture.hub),
        auth: Arc::clone(&b.auth),
        pending_link: None,
        processes: Arc::clone(&fixture.hub.processes),
        approvals: Arc::clone(&fixture.approvals),
        compare: compare::CompareState::new(
            fixture._dirs[0].path().join("compare"),
            aura_gate::compare::DEFAULT_CONCURRENCY,
        ),
        flow: None,
        cloud: crate::event_loop::cloud::CloudState::default(),
        open_channel: None,
        side_panels: None,
    }
}

pub(super) fn lock_registry_agent(
    fixture: &Fixture,
    agent_id: AgentId,
) -> crate::agent_registry::RegisteredAgent {
    subagents::lock(&fixture.hub.registry)
        .find(agent_id)
        .cloned()
        .unwrap()
}

pub(super) fn message(agent_id: AgentId, text: &str, shown: bool) -> UiEvent {
    UiEvent::UserMessage {
        agent_id: agent_id.to_hex(),
        text: text.to_string(),
        shown,
        images: Vec::new(),
    }
}

fn png(data: &str) -> aura_ui::events::ImageAttachment {
    aura_ui::events::ImageAttachment {
        media_type: "image/png".to_string(),
        data: data.to_string(),
        bytes: 3,
    }
}

pub(super) fn message_with_image(agent_id: AgentId, text: &str, shown: bool) -> UiEvent {
    UiEvent::UserMessage {
        agent_id: agent_id.to_hex(),
        text: text.to_string(),
        shown,
        images: vec![png("aGk=")],
    }
}

/// Media types of the image blocks in the user messages of `agent_id`'s
/// idle session.
pub(super) fn user_images(state: &LoopState<'_>, agent_id: AgentId) -> Vec<(String, String)> {
    state.idle[&agent_id]
        .messages
        .iter()
        .filter(|m| m.role == aura_model::Role::User)
        .flat_map(|m| m.content.iter())
        .filter_map(|block| match block {
            aura_model::ContentBlock::Image { source } => {
                Some((source.media_type.clone(), source.data.clone()))
            }
            _ => None,
        })
        .collect()
}

pub(super) async fn next_done(done: &mut mpsc::Receiver<WorkDone>) -> WorkDone {
    tokio::time::timeout(Duration::from_secs(10), done.recv())
        .await
        .expect("agent work did not finish")
        .expect("work channel closed")
}

/// Wait for the job filling the side panels of the agent on screen, so
/// [`drain`] gets what it sends too.
pub(super) async fn settle(state: &mut LoopState<'_>) {
    if let Some(job) = state.side_panels.take() {
        job.await.unwrap();
    }
}

/// Commands sent so far, with the agent each was addressed to.
pub(super) fn drain(commands: &mut mpsc::Receiver<UiCommand>) -> Vec<(Option<String>, UiCommand)> {
    let mut out = Vec::new();
    while let Ok(command) = commands.try_recv() {
        match command {
            UiCommand::ForAgent { agent_id, command } => out.push((Some(agent_id), *command)),
            other => out.push((None, other)),
        }
    }
    out
}

pub(super) fn completed(commands: &[(Option<String>, UiCommand)], agent_id: AgentId) -> usize {
    let hex = agent_id.to_hex();
    commands
        .iter()
        .filter(|(to, c)| to.as_deref() == Some(hex.as_str()) && matches!(c, UiCommand::Complete))
        .count()
}
