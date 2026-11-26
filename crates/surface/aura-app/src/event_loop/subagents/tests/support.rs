use super::*;


pub(in crate::event_loop) struct Fixture {
    pub(in crate::event_loop) hub: Arc<AgentHub>,
    pub(in crate::event_loop) dispatcher: Arc<dyn SubagentDispatchHook>,
    pub(super) commands: mpsc::Receiver<UiCommand>,
    pub(super) trace_store: Arc<aura_store_db::TraceStore>,
    pub(super) _data: TempDir,
    _db: TempDir,
    pub(super) workspace: TempDir,
}

/// A hub over seeded permanent agents whose kernels all answer `reply`.
pub(in crate::event_loop) fn fixture(reply: &str) -> Fixture {
    slow_fixture(reply, 0)
}

/// [`fixture`] whose model takes `latency_ms` to answer.
pub(super) fn slow_fixture(reply: &str, latency_ms: u64) -> Fixture {
    fixture_with(reply, latency_ms, "")
}

/// [`fixture`] that names subtasks with `title_model`, Gate's classifier
/// (none when empty). With a title model every call is answered `reply`,
/// so the subtitle call and the turn can run in either order.
pub(super) fn titled_fixture(reply: &str, title_model: &str) -> Fixture {
    fixture_with(reply, 0, title_model)
}

fn fixture_with(reply: &str, latency_ms: u64, title_model: &str) -> Fixture {
    let data = TempDir::new().unwrap();
    let db = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let store = Arc::new(RocksStore::open(db.path(), false).unwrap());
    let (tx, commands) = mpsc::channel(64);
    let trace_store = Arc::new(aura_store_db::TraceStore::for_store(&store));
    let parts = hub_parts(&store, &data, &workspace, title_model, tx, &trace_store);
    let answers = MockAnswers {
        reply: reply.to_string(),
        latency_ms,
        answers_all: !title_model.is_empty(),
    };
    let workspace_root = workspace.path().to_path_buf();
    let mut captured = None;
    let hub = AgentHub::new(parts, |dispatcher| {
        captured = Some(dispatcher);
        kernel_factory(store, answers, workspace_root)
    });
    Fixture {
        hub,
        dispatcher: captured.unwrap(),
        commands,
        trace_store,
        _data: data,
        _db: db,
        workspace,
    }
}

/// How the fixture's mock models answer.
struct MockAnswers {
    reply: String,
    latency_ms: u64,
    /// Answer every call `reply`, not just the first.
    answers_all: bool,
}

/// The hub's parts over `store`, with the default tools minus the task tool.
fn hub_parts(
    store: &Arc<RocksStore>,
    data: &TempDir,
    workspace: &TempDir,
    title_model: &str,
    tx: mpsc::Sender<UiCommand>,
    trace_store: &Arc<aura_store_db::TraceStore>,
) -> AgentHubParts {
    let traces = Traces::new(
        Some(data.path().join("home")),
        Some(Arc::clone(trace_store)),
        None,
        tx.clone(),
    );
    AgentHubParts {
        // On the kernels' database, so channel rows and the registry's
        // agent rows share it as they do in the app.
        registry: AgentRegistry::open(
            aura_store_db::AgentStore::for_store(store),
            data.path(),
            workspace.path(),
        )
        .unwrap(),
        definitions: AgentDefinitions::load_or_seed(data.path()).unwrap(),
        skills: SkillManager::new(SkillLoader::with_defaults(
            Some(workspace.path().to_path_buf()),
            None,
        )),
        base: turn_base(title_model),
        commands: tx,
        long_run: crate::long_run::LongRunSettings::default(),
        mode: aura_ui::PermissionMode::Agent,
        approval: aura_ui::ApprovalLevel::Ask,
        approvals: None,
        instructions: ProjectInstructions::only_in(workspace.path()),
        processes: ProcessTable::without_os_scan(None, None),
        agents: None,
        record_panel: Default::default(),
        gate: Default::default(),
        traces,
        channels: Some(aura_store_db::ChannelStore::for_store(store)),
    }
}

/// The turn base: the default config and tools minus the task tool, naming
/// subtasks with `title_model`.
fn turn_base(title_model: &str) -> TurnBase {
    let (_, tools) = build_executor_router_with_config(&ToolConfig::for_autonomous_dev_loop());
    let config = default_agent_config_with_auth("mock-model", None);
    TurnBase {
        limits: ModelLimits::of(&config),
        config,
        tools: tools
            .into_iter()
            .filter(|t| t.name != TASK_TOOL_NAME)
            .collect(),
        gate: GateBase {
            classifier: title_model.to_string(),
            candidates: None,
        },
    }
}

/// Kernels over `store` whose mock models answer as `answers` says, working
/// in their folder or else `workspace_root`.
fn kernel_factory(
    store: Arc<RocksStore>,
    answers: MockAnswers,
    workspace_root: std::path::PathBuf,
) -> KernelFactory {
    let kernel_store = Arc::clone(&store);
    let MockAnswers {
        reply,
        latency_ms,
        answers_all,
    } = answers;
    KernelFactory::new(
        store,
        Box::new(move |agent_id, _parent_chain, folder| {
            let mut mock = MockProvider::simple_response(reply.clone()).with_latency(latency_ms);
            if answers_all {
                mock = mock.with_default_response(MockResponse::text(reply.clone()));
            }
            let provider: Arc<dyn ModelProvider + Send + Sync> = Arc::new(mock);
            Ok(Kernel::new(
                Arc::clone(&kernel_store) as Arc<dyn aura_store_db::Store>,
                provider,
                build_executor_router_with_config(&ToolConfig::for_autonomous_dev_loop()).0,
                KernelConfig {
                    workspace_base: folder.unwrap_or_else(|| workspace_root.clone()),
                    use_workspace_base_as_root: true,
                    ..KernelConfig::default()
                },
                agent_id,
            )?)
        }),
    )
}

impl Fixture {
    /// Take every UI command sent so far; returns how many there were.
    /// Nothing else reads the command channel in these tests, so a test
    /// that spawns many agents drains it to keep `send().await` from
    /// waiting on a full channel.
    pub(in crate::event_loop) fn drain_commands(&mut self) -> usize {
        let mut count = 0;
        while self.commands.try_recv().is_ok() {
            count += 1;
        }
        count
    }

    /// Take every UI command sent so far, in the order they were sent.
    pub(in crate::event_loop) fn take_commands(&mut self) -> Vec<UiCommand> {
        let mut out = Vec::new();
        while let Ok(command) = self.commands.try_recv() {
            out.push(command);
        }
        out
    }
}

pub(in crate::event_loop) fn request(
    parent: AgentId,
    subagent_type: &str,
    prompt: &str,
) -> SubagentDispatchRequest {
    serde_json::from_value(serde_json::json!({
        "parent_agent_id": parent,
        "subagent_type": subagent_type,
        "prompt": prompt,
        "parent_permissions": AgentPermissions::full_access(),
        "user_tool_defaults": UserToolDefaults::default(),
    }))
    .unwrap()
}
