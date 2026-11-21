use super::*;
use crate::session_helpers::{build_executor_router_with_config, default_agent_config_with_auth};
use aura_agent_kernel::{Kernel, KernelConfig};
use aura_context_skills::SkillLoader;
use aura_core_types::{AgentPermissions, UserToolDefaults};
use aura_exec_tools::ToolConfig;
use aura_model::{MockProvider, ModelProvider};
use aura_store_db::RocksStore;
use tempfile::TempDir;

struct Fixture {
    hub: Arc<AgentHub>,
    dispatcher: Arc<dyn SubagentDispatchHook>,
    commands: mpsc::Receiver<UiCommand>,
    trace_store: Arc<aura_store_db::TraceStore>,
    _data: TempDir,
    _db: TempDir,
    workspace: TempDir,
}

/// A hub over seeded permanent agents whose kernels all answer `reply`.
fn fixture(reply: &str) -> Fixture {
    slow_fixture(reply, 0)
}

/// [`fixture`] whose model takes `latency_ms` to answer.
fn slow_fixture(reply: &str, latency_ms: u64) -> Fixture {
    let data = TempDir::new().unwrap();
    let db = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let store = Arc::new(RocksStore::open(db.path(), false).unwrap());
    let (_, tools) = build_executor_router_with_config(&ToolConfig::for_autonomous_dev_loop());
    let config = default_agent_config_with_auth("mock-model", None);
    let (tx, commands) = mpsc::channel(64);
    let trace_store = Arc::new(aura_store_db::TraceStore::for_store(&store));
    let traces = Traces::new(
        Some(data.path().join("home")),
        Some(Arc::clone(&trace_store)),
        None,
        tx.clone(),
    );
    let parts = AgentHubParts {
        registry: AgentRegistry::load_or_init_in(data.path(), workspace.path()).unwrap(),
        definitions: AgentDefinitions::load_or_seed(data.path()).unwrap(),
        skills: SkillManager::new(SkillLoader::with_defaults(
            Some(workspace.path().to_path_buf()),
            None,
        )),
        base: TurnBase {
            limits: ModelLimits::of(&config),
            config,
            tools: tools
                .into_iter()
                .filter(|t| t.name != TASK_TOOL_NAME)
                .collect(),
            gate: GateBase::default(),
        },
        commands: tx,
        long_run: crate::long_run::LongRunSettings::default(),
        mode: aura_ui_terminal::PermissionMode::Agent,
        approval: aura_ui_terminal::ApprovalLevel::Ask,
        approvals: None,
        instructions: ProjectInstructions::only_in(workspace.path()),
        processes: ProcessTable::without_os_scan(None, None),
        agents: None,
        record_panel: Default::default(),
        gate: Default::default(),
        traces,
    };
    let reply = reply.to_string();
    let workspace_root = workspace.path().to_path_buf();
    let mut captured = None;
    let hub = AgentHub::new(parts, |dispatcher| {
        captured = Some(dispatcher);
        let kernel_store = Arc::clone(&store);
        KernelFactory::new(
            store,
            Box::new(move |agent_id, _parent_chain, folder| {
                let provider: Arc<dyn ModelProvider + Send + Sync> =
                    Arc::new(MockProvider::simple_response(reply.clone()).with_latency(latency_ms));
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

fn request(parent: AgentId, subagent_type: &str, prompt: &str) -> SubagentDispatchRequest {
    serde_json::from_value(serde_json::json!({
        "parent_agent_id": parent,
        "subagent_type": subagent_type,
        "prompt": prompt,
        "parent_permissions": AgentPermissions::full_access(),
        "user_tool_defaults": UserToolDefaults::default(),
    }))
    .unwrap()
}

#[test]
fn task_tool_lists_permanent_agents() {
    let fixture = fixture("unused");
    let tool = task_tool_definition(&lock(&fixture.hub.definitions)).unwrap();

    assert_eq!(tool.name, TASK_TOOL_NAME);
    assert_eq!(
        tool.input_schema["properties"]["subagent_type"]["enum"],
        serde_json::json!([
            "engineer",
            "explorer",
            "frontend-engineer",
            "implement",
            "product-manager",
            "reviewer",
            "rust-engineer",
            "tests",
            "ui-designer",
            "web-designer"
        ])
    );
    assert!(tool.description.contains("- reviewer: "));
    assert!(tool
        .description
        .contains("- ui-designer: Product UI designer"));
}

#[test]
fn no_task_tool_without_permanent_agents() {
    let data = TempDir::new().unwrap();
    let mut definitions = AgentDefinitions::load_or_seed(data.path()).unwrap();
    let names: Vec<String> = definitions.all().iter().map(|d| d.name.clone()).collect();
    for name in names {
        definitions.delete(&name).unwrap();
    }

    assert!(task_tool_definition(&definitions).is_none());
}

#[test]
fn turn_applies_definition_rules_and_tools() {
    let fixture = fixture("unused");
    let workspace = fixture.workspace.path();

    let (_, plain_tools) = fixture.hub.turn(None, workspace);
    assert!(plain_tools.iter().any(|t| t.name == TASK_TOOL_NAME));

    let (config, tools) = fixture.hub.turn(Some("explorer"), workspace);
    assert!(config
        .system_prompt
        .contains("<agent_definition name=\"explorer\">"));
    assert!(tools.iter().all(|t| t.name != TASK_TOOL_NAME));
    assert!(tools.iter().any(|t| t.name == "read_file"));
    assert!(tools.iter().all(|t| t.name != "write_file"));

    let (config, tools) = fixture.hub.turn(Some("ui-designer"), workspace);
    assert!(config.system_prompt.contains("--accent-ink: #1a1a1a;"));
    assert!(tools.iter().any(|t| t.name == "write_file"));
    assert!(tools.iter().all(|t| t.name != TASK_TOOL_NAME));
}

#[test]
fn turn_puts_project_instructions_in_the_stable_prompt_prefix() {
    let fixture = fixture("unused");
    let workspace = fixture.workspace.path();
    let base = lock(&fixture.hub.base).config.system_prompt.clone();
    let (config, _) = fixture.hub.turn(Some("explorer"), workspace);
    assert!(!config.system_prompt.contains("<project_instructions>"));

    std::fs::write(
        workspace.join("AGENTS.md"),
        "Run `make check` before committing.",
    )
    .unwrap();
    let (config, _) = fixture.hub.turn(Some("explorer"), workspace);
    let prompt = config.system_prompt;
    let instructions = prompt.find("<project_instructions>").unwrap();
    let definition = prompt.find("<agent_definition").unwrap();
    assert!(prompt.starts_with(&format!("{base}\n\n<project_instructions>")));
    assert!(instructions < definition);
    assert!(prompt.contains("Run `make check` before committing."));

    let (again, _) = fixture.hub.turn(Some("explorer"), workspace);
    assert_eq!(
        again.system_prompt, prompt,
        "unchanged files keep the prompt cacheable"
    );
}

#[tokio::test]
async fn unknown_permanent_agent_is_rejected() {
    let fixture = fixture("unused");
    let parent = lock(&fixture.hub.registry).active().agent_id();

    let result = fixture
        .dispatcher
        .dispatch(request(parent, "nobody", "hi"))
        .await
        .unwrap();

    assert!(
        matches!(&result.exit, SubagentExit::Rejected { reason } if reason.contains("explorer, frontend-engineer"))
    );
    assert_eq!(lock(&fixture.hub.registry).agents().len(), 1);
}

#[tokio::test]
async fn spawn_is_recorded_as_a_run_linked_to_its_parent() {
    let fixture = fixture("Looks good.");
    let parent = lock(&fixture.hub.registry).active().agent_id();
    let parent_run = fixture
        .hub
        .traces
        .start(parent, RunMeta::new("mock-model", RunSource::Tui))
        .unwrap()
        .run_id();

    let result = fixture
        .dispatcher
        .dispatch(request(parent, "reviewer", "Review the diff"))
        .await
        .unwrap();

    let child = result.child_agent_id.unwrap();
    let trace = aura_agent_trace::db::latest_of(&fixture.trace_store, &child.to_hex())
        .unwrap()
        .expect("the spawn's run is in the database");
    assert_eq!(trace.meta.spawned_by.as_deref(), Some(parent_run.as_str()));
    assert_eq!(trace.meta.title, "Review the diff");
    assert_eq!(trace.meta.mode, "reviewer");
    assert!(trace.scorecard().turns >= 1);
}

#[tokio::test]
async fn spawn_runs_an_instance_under_its_parent() {
    let mut fixture = fixture("Looks good.");
    let parent = lock(&fixture.hub.registry).active().clone();

    let result = fixture
        .dispatcher
        .dispatch(request(parent.agent_id(), "reviewer", "Review the diff"))
        .await
        .unwrap();

    assert!(matches!(result.exit, SubagentExit::Completed));
    assert_eq!(result.final_message, "Looks good.");
    let child_id = result.child_agent_id.unwrap();
    {
        let registry = lock(&fixture.hub.registry);
        let child = registry.find(child_id).unwrap();
        assert_eq!(child.name, "reviewer #1");
        assert_eq!(child.definition.as_deref(), Some("reviewer"));
        assert_eq!(child.parent.as_deref(), Some(parent.zns_id.as_str()));
        assert_eq!(registry.active().agent_id(), parent.agent_id());
    }

    let finished = fixture.hub.take_finished();
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].agent_id(), child_id);
    assert_eq!(finished[0].definition.as_deref(), Some("reviewer"));
    assert_eq!(finished[0].messages.len(), 2);
    assert!(fixture.hub.take_finished().is_empty());

    let summaries = fixture.hub.agent_summaries();
    let child_row = summaries.iter().find(|s| s.name == "reviewer #1").unwrap();
    assert_eq!(child_row.depth, 1);
    assert!(!child_row.running);

    let mut saw_running = false;
    let mut child_chat = Vec::new();
    while let Ok(command) = fixture.commands.try_recv() {
        match command {
            UiCommand::SetAgents(rows) => {
                saw_running |= rows.iter().any(|r| r.name == "reviewer #1" && r.running);
            }
            UiCommand::ForAgent { agent_id, command } if agent_id == child_id.to_hex() => {
                child_chat.push(*command);
            }
            _ => {}
        }
    }
    assert!(saw_running);
    assert!(matches!(
        child_chat.first(),
        Some(UiCommand::SetInstructions(_))
    ));
    assert!(matches!(
        child_chat.get(1),
        Some(UiCommand::TurnStarted { prompt: Some(p) }) if p == "Review the diff"
    ));
    assert!(matches!(child_chat.last(), Some(UiCommand::Complete)));
}

#[tokio::test]
async fn cancelling_a_spawn_fails_its_task() {
    let fixture = slow_fixture("unused", 60_000);
    let parent = lock(&fixture.hub.registry).active().agent_id();
    let hub = Arc::clone(&fixture.hub);

    let dispatch = tokio::spawn({
        let dispatcher = Arc::clone(&fixture.dispatcher);
        async move {
            dispatcher
                .dispatch(request(parent, "reviewer", "Review it"))
                .await
                .unwrap()
        }
    });
    let child = loop {
        let running = lock(&hub.running).keys().next().copied();
        if let Some(child) = running {
            break child;
        }
        tokio::task::yield_now().await;
    };
    assert!(hub.cancel(child));

    let result = tokio::time::timeout(std::time::Duration::from_secs(10), dispatch)
        .await
        .expect("cancel stops the spawn")
        .unwrap();
    assert!(
        matches!(&result.exit, SubagentExit::Failed { reason } if reason == "cancelled by the user")
    );
    assert!(!hub.is_running(child));
    assert_eq!(hub.take_finished().len(), 1);
}

#[tokio::test]
async fn a_writer_waiting_for_the_workspace_tells_its_chat_it_is_queued() {
    let mut fixture = slow_fixture("unused", 60_000);
    let parent = lock(&fixture.hub.registry).active().agent_id();
    let hub = Arc::clone(&fixture.hub);
    for prompt in ["first edit", "second edit"] {
        let dispatcher = Arc::clone(&fixture.dispatcher);
        let asked = request(parent, "engineer", prompt);
        tokio::spawn(async move { dispatcher.dispatch(asked).await });
    }

    let mut queued = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !queued && std::time::Instant::now() < deadline {
        while let Ok(command) = fixture.commands.try_recv() {
            if let UiCommand::ForAgent { command, .. } = command {
                queued |= matches!(
                    *command,
                    UiCommand::ShowMessage(ref m) if m.content.starts_with("Queued:")
                );
            }
        }
        tokio::task::yield_now().await;
    }
    assert!(queued, "the waiting agent's chat says why it is empty");

    let running: Vec<AgentId> = lock(&hub.running).keys().copied().collect();
    for child in running {
        hub.cancel(child);
    }
}

#[tokio::test]
async fn waiting_spawn_is_listed_as_a_finished_task() {
    let fixture = fixture("Looks good.");
    let parent = lock(&fixture.hub.registry).active().agent_id();

    let result = fixture
        .dispatcher
        .dispatch(request(parent, "reviewer", "Review the diff"))
        .await
        .unwrap();

    let rows = fixture.hub.processes.snapshot();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, task_id(result.child_agent_id.unwrap()));
    assert_eq!(rows[0].kind, ProcessKind::Task);
    assert_eq!(rows[0].status, ProcessStatus::Exited);
    assert_eq!(rows[0].owner_agent_id, parent.to_hex());
    assert_eq!(rows[0].detail, "Review the diff");
    assert_eq!(
        rows[0].jump_agent_id,
        result.child_agent_id.unwrap().to_hex()
    );
}

#[tokio::test]
async fn detached_spawn_returns_at_once_and_replies_with_a_notice() {
    let fixture = fixture("Looks good.");
    let parent = lock(&fixture.hub.registry).active().agent_id();
    let mut notices = fixture.hub.take_notices();
    let mut detached = request(parent, "reviewer", "Review the diff");
    detached.spawn_mode = Some(SpawnMode::Detached);

    let result = fixture.dispatcher.dispatch(detached).await.unwrap();
    assert!(matches!(result.exit, SubagentExit::Completed));
    assert!(result.final_message.contains("in the background"));
    let child = result.child_agent_id.unwrap();

    let notice = tokio::time::timeout(std::time::Duration::from_secs(10), notices.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(notice.to, parent);
    assert_eq!(notice.shown, "reviewer #1 finished its background task");
    assert!(notice.prompt.ends_with("Its reply:\n\nLooks good."));

    assert!(!fixture.hub.is_running(child));
    let finished = fixture.hub.take_finished();
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].agent_id(), child);
    let rows = fixture.hub.processes.snapshot();
    assert_eq!(rows[0].kind, ProcessKind::Agent);
    assert_eq!(rows[0].status, ProcessStatus::Exited);
}

#[test]
fn task_tool_offers_detached_spawns() {
    let fixture = fixture("unused");
    let tool = task_tool_definition(&lock(&fixture.hub.definitions)).unwrap();
    assert_eq!(
        tool.input_schema["properties"]["spawn_mode"]["enum"],
        serde_json::json!(["wait", "detached"])
    );
}

#[tokio::test]
async fn spawned_instance_works_in_its_projects_folder() {
    let fixture = fixture("Done.");
    let folder = TempDir::new().unwrap();
    let (parent, _) = lock(&fixture.hub.registry)
        .open_project(folder.path())
        .unwrap();

    let result = fixture
        .dispatcher
        .dispatch(request(parent.agent_id(), "reviewer", "Review it"))
        .await
        .unwrap();

    assert!(matches!(result.exit, SubagentExit::Completed));
    let finished = fixture.hub.take_finished();
    assert_eq!(
        finished[0].kernel.agent_workspace(),
        folder.path().canonicalize().unwrap()
    );
}

/// A registered instance of `definition`, opened.
fn instance(fixture: &Fixture, definition: &str) -> AgentSession {
    let agent = lock(&fixture.hub.registry)
        .create_instance(definition, None)
        .unwrap();
    fixture.hub.factory.open(&agent, Vec::new(), None).unwrap()
}

#[tokio::test]
async fn unstarted_spawns_are_archived_but_started_ones_are_left_alone() {
    let fixture = fixture("Looks good.");
    let parent = lock(&fixture.hub.registry).active().agent_id();
    let parent_zns = lock(&fixture.hub.registry)
        .find(parent)
        .unwrap()
        .zns_id
        .clone();

    // Never opened: the app closed before a `task` call got anywhere.
    let unstarted = lock(&fixture.hub.registry)
        .create_instance("reviewer", Some(parent_zns.clone()))
        .unwrap();
    // Opened and ran a turn: there is a record to show and resume.
    let started = lock(&fixture.hub.registry)
        .create_instance("reviewer", Some(parent_zns))
        .unwrap();
    let session = fixture
        .hub
        .factory
        .open(&started, Vec::new(), None)
        .unwrap();
    crate::event_loop::handlers::mark_session_start(&session)
        .await
        .unwrap();

    fixture.hub.archive_unstarted_spawns();

    let registry = lock(&fixture.hub.registry);
    assert!(registry.find(unstarted.agent_id()).unwrap().archived);
    assert_eq!(
        registry
            .find(unstarted.agent_id())
            .unwrap()
            .title
            .as_deref(),
        Some("Did not start before the app closed")
    );
    assert!(!registry.find(started.agent_id()).unwrap().archived);
    // A project the user created, not a `task` spawn, is never touched
    // even though it has no record yet.
    assert!(!registry.find(parent).unwrap().archived);
}

fn attempt(session: &AgentSession, kind: &str, prompt: &str) -> Attempt {
    Attempt {
        kind: kind.to_string(),
        prompt: prompt.to_string(),
        child: session.agent_id(),
        kernel: Arc::clone(&session.kernel),
        ended: None,
        redone: false,
    }
}

#[tokio::test]
async fn a_subtask_redone_while_still_running_fails_once_it_ends() {
    let fixture = fixture("unused");
    let parent = lock(&fixture.hub.registry).active().agent_id();
    let parent_session = fixture
        .hub
        .factory
        .open(
            &lock(&fixture.hub.registry).active().clone(),
            Vec::new(),
            None,
        )
        .unwrap();
    let (first, second) = (
        instance(&fixture, "explorer"),
        instance(&fixture, "explorer"),
    );
    let prompt = "Find the config loader";
    assert!(fixture
        .hub
        .begin_attempt(parent, attempt(&first, "explorer", prompt))
        .is_empty());
    fixture.hub.start_running(
        parent,
        Arc::clone(&parent_session.kernel),
        CancellationToken::new(),
        None,
    );
    assert!(fixture
        .hub
        .begin_attempt(parent, attempt(&first, "explorer", prompt))
        .is_empty());
    assert!(fixture
        .hub
        .begin_attempt(parent, attempt(&second, "explorer", prompt))
        .is_empty());

    let ended = OutcomePayload {
        prompt_tx: Hash::from_content(b"prompt"),
        category: PromptCategory::Research,
        model: "m".to_string(),
        success: None,
        signal: OutcomeSignal::Unavailable,
        cost_usd: 0.0,
        subtask_cost_usd: 0.0,
        latency_ms: 1,
    };
    let redo = fixture
        .hub
        .end_attempt(parent, first.agent_id(), Some(ended.clone()))
        .expect("the first attempt was redone while it ran");
    redo.record().await;
    assert!(fixture
        .hub
        .end_attempt(parent, second.agent_id(), Some(ended))
        .is_none());
    let outcomes: Vec<OutcomePayload> = fixture
        .hub
        .factory
        .store
        .scan_record(first.agent_id(), 1, 100)
        .unwrap()
        .iter()
        .filter_map(|e| e.tx.outcome_payload())
        .collect();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].signal, OutcomeSignal::Redone);
    assert_eq!(outcomes[0].success, Some(false));
}

/// The status of the Processes tab row of `child`'s task, once it has one.
fn task_row_status(hub: &AgentHub, child: AgentId) -> Option<ProcessStatus> {
    let id = task_id(child);
    hub.processes
        .snapshot()
        .into_iter()
        .find(|row| row.id == id)
        .map(|row| row.status)
}

#[tokio::test]
async fn dropping_a_waiting_spawn_mid_run_stops_it_running() {
    let fixture = slow_fixture("unused", 60_000);
    let parent = lock(&fixture.hub.registry).active().agent_id();
    let hub = Arc::clone(&fixture.hub);

    // Stands in for the parent's tool call, which cancelling the parent's
    // turn drops without cancelling the spawn's own token.
    let dispatch = tokio::spawn({
        let dispatcher = Arc::clone(&fixture.dispatcher);
        async move {
            dispatcher
                .dispatch(request(parent, "reviewer", "Review it"))
                .await
        }
    });
    let child = loop {
        let running = lock(&hub.running).keys().next().copied();
        if let Some(child) = running.filter(|child| task_row_status(&hub, *child).is_some()) {
            break child;
        }
        tokio::task::yield_now().await;
    };
    dispatch.abort();
    let joined = dispatch.await;
    assert!(joined.is_err_and(|e| e.is_cancelled()));

    assert!(!hub.is_running(child));
    assert_eq!(hub.take_abandoned(), [child]);
    assert!(hub.take_finished().is_empty(), "its session went with it");
    assert_eq!(task_row_status(&hub, child), Some(ProcessStatus::Killed));
}

#[tokio::test]
async fn a_spawn_that_ends_normally_is_cleaned_up_once() {
    let fixture = fixture("Looks good.");
    let parent = lock(&fixture.hub.registry).active().agent_id();

    let result = fixture
        .dispatcher
        .dispatch(request(parent, "reviewer", "Review the diff"))
        .await
        .unwrap();

    let child = result.child_agent_id.unwrap();
    assert!(!fixture.hub.is_running(child));
    assert_eq!(fixture.hub.take_finished().len(), 1);
    assert!(
        fixture.hub.take_abandoned().is_empty(),
        "the disarmed guard did nothing on drop"
    );
    assert_eq!(
        task_row_status(&fixture.hub, child),
        Some(ProcessStatus::Exited)
    );
}

#[tokio::test]
async fn an_armed_guard_ends_only_the_running_it_started() {
    let fixture = fixture("unused");
    let session = instance(&fixture, "explorer");
    let agent = session.agent_id();
    let kernel = || Arc::clone(&session.kernel);

    let guard = fixture
        .hub
        .start_guarded(agent, kernel(), CancellationToken::new());
    drop(guard);
    assert!(!fixture.hub.is_running(agent));
    assert_eq!(fixture.hub.take_abandoned(), [agent]);

    // A stale guard whose agent started running again since.
    let stale = fixture
        .hub
        .start_guarded(agent, kernel(), CancellationToken::new());
    fixture
        .hub
        .start_running(agent, kernel(), CancellationToken::new(), None);
    drop(stale);
    assert!(fixture.hub.is_running(agent), "the newer turn survives");
    assert!(fixture.hub.take_abandoned().is_empty());
    fixture.hub.stop_running(agent);

    // A disarmed guard leaves ending the running to its owner.
    let disarmed = fixture
        .hub
        .start_guarded(agent, kernel(), CancellationToken::new());
    disarmed.disarm();
    assert!(fixture.hub.is_running(agent));
    assert!(fixture.hub.take_abandoned().is_empty());
}
