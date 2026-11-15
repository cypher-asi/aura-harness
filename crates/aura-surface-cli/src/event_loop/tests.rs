use super::*;
use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::session_helpers::{build_executor_router_with_config, default_agent_config_with_auth};
use async_trait::async_trait;
use aura_agent::AgentLoopConfig;
use aura_agent_kernel::KernelConfig;
use aura_auth::{Vault, ZosIdentity};
use aura_context_skills::{SkillLoader, SkillManager};
use aura_model_reasoner::{
    MockProvider, MockResponse, ModelProvider, ModelRequest, ModelResponse, ReasonerError,
};
use aura_store_db::RocksStore;
use aura_terminal::events::RewindScope;
use aura_terminal::PermissionMode;
use aura_tools::ToolConfig;
use tempfile::TempDir;
use tokio::sync::Semaphore;

/// Answers like a mock model, but holds calls whose last message mentions
/// "slow" until the test releases them.
struct GatedProvider {
    inner: MockProvider,
    gate: Arc<Semaphore>,
}

#[async_trait]
impl ModelProvider for GatedProvider {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        let last = request.messages.last().map(|m| m.text_content());
        let slow = last.as_deref().is_some_and(|text| text.contains("slow"));
        if slow {
            self.gate.acquire().await.unwrap().forget();
        }
        if last.as_deref().is_some_and(|text| text.contains("run ls")) {
            return MockProvider::new()
                .with_response(MockResponse::tool_use(
                    "toolu_ls",
                    "run_command",
                    serde_json::json!({ "program": "ls", "args": ["."] }),
                ))
                .complete(request)
                .await;
        }
        self.inner.complete(request).await
    }

    async fn health_check(&self) -> bool {
        true
    }
}

struct Fixture {
    hub: Arc<AgentHub>,
    config: AgentLoopConfig,
    gate: Arc<Semaphore>,
    commands_tx: mpsc::Sender<UiCommand>,
    approvals: Arc<TerminalApprovals>,
    first: AgentId,
    second: AgentId,
    files: crate::file_checkpoints::FileCheckpoints,
    workspace: std::path::PathBuf,
    _dirs: Vec<TempDir>,
}

fn fixture() -> (Fixture, mpsc::Receiver<UiCommand>) {
    let data = TempDir::new().unwrap();
    let db = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let store = Arc::new(RocksStore::open(db.path(), false).unwrap());
    let (_, tools) = build_executor_router_with_config(&ToolConfig::for_autonomous_dev_loop());
    let config = default_agent_config_with_auth("mock-model", None);
    let (commands_tx, commands) = mpsc::channel(4096);
    let mut registry = AgentRegistry::load_or_init(data.path()).unwrap();
    let first = registry.active().agent_id();
    let second = registry
        .create(Some("Second".to_string()))
        .unwrap()
        .agent_id();
    let parts = AgentHubParts {
        registry,
        definitions: AgentDefinitions::load_or_seed(data.path()).unwrap(),
        skills: SkillManager::new(SkillLoader::with_defaults(
            Some(workspace.path().to_path_buf()),
            None,
        )),
        base: TurnBase {
            limits: crate::model_selection::ModelLimits::of(&config),
            config: config.clone(),
            tools,
        },
        commands: commands_tx.clone(),
        long_run: crate::long_run::LongRunSettings::default(),
        mode: PermissionMode::Default,
        instructions: crate::project_instructions::ProjectInstructions::only_in(workspace.path()),
    };
    let gate = Arc::new(Semaphore::new(0));
    let provider_gate = Arc::clone(&gate);
    let workspace_root = workspace.path().to_path_buf();
    let checkpoints = crate::session_checkpoint::Checkpoints::new(data.path());
    let approvals = Arc::new(TerminalApprovals::new(commands_tx.clone(), data.path()));
    let prompter = Arc::clone(&approvals);
    let files = crate::file_checkpoints::FileCheckpoints::new(data.path());
    let factory_files = files.clone();
    let hub = AgentHub::new(parts, |_dispatcher| {
        let kernel_store = Arc::clone(&store);
        KernelFactory::new(
            store,
            Box::new(move |agent_id, _parent_chain, folder| {
                let provider: Arc<dyn ModelProvider + Send + Sync> = Arc::new(GatedProvider {
                    inner: MockProvider::simple_response("Done."),
                    gate: Arc::clone(&provider_gate),
                });
                let workspace_base = folder.unwrap_or_else(|| workspace_root.clone());
                prompter.set_folder(agent_id, workspace_base.clone());
                Ok(Kernel::new(
                    Arc::clone(&kernel_store) as Arc<dyn aura_store_db::Store>,
                    provider,
                    build_executor_router_with_config(&ToolConfig::for_autonomous_dev_loop()).0,
                    KernelConfig {
                        workspace_base,
                        use_workspace_base_as_root: true,
                        tool_approval_prompter: Some(Arc::clone(&prompter)
                            as Arc<dyn aura_agent_kernel::ToolApprovalPrompter>),
                        ..KernelConfig::default()
                    },
                    agent_id,
                )?)
            }),
        )
        .with_checkpoints(checkpoints)
        .with_file_checkpoints(factory_files)
    });
    let fixture = Fixture {
        hub,
        config,
        gate,
        commands_tx,
        approvals,
        first,
        second,
        files,
        workspace: workspace.path().canonicalize().unwrap(),
        _dirs: vec![data, db, workspace],
    };
    (fixture, commands)
}

/// Everything a [`LoopState`] borrows.
struct Borrowed {
    events: mpsc::Receiver<UiEvent>,
    agent_loop: AgentLoop,
    done: mpsc::Receiver<WorkDone>,
    done_tx: mpsc::Sender<WorkDone>,
    links: mpsc::Sender<auth::LinkOutcome>,
    _auth_dir: TempDir,
    auth: Arc<AuthManager>,
}

fn borrowed(fixture: &Fixture) -> Borrowed {
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

fn loop_state<'a>(fixture: &'a Fixture, b: &'a mut Borrowed) -> LoopState<'a> {
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
        queued: HashMap::new(),
        work_done: b.done_tx.clone(),
        in_flight: 0,
        hub: Arc::clone(&fixture.hub),
        auth: Arc::clone(&b.auth),
        pending_link: None,
        jobs: BackgroundJobs::new(mpsc::channel(1).0),
        approvals: Arc::clone(&fixture.approvals),
    }
}

fn lock_registry_agent(
    fixture: &Fixture,
    agent_id: AgentId,
) -> crate::agent_registry::RegisteredAgent {
    subagents::lock(&fixture.hub.registry)
        .find(agent_id)
        .cloned()
        .unwrap()
}

fn message(agent_id: AgentId, text: &str, shown: bool) -> UiEvent {
    UiEvent::UserMessage {
        agent_id: agent_id.to_hex(),
        text: text.to_string(),
        shown,
        images: Vec::new(),
    }
}

fn png(data: &str) -> aura_terminal::events::ImageAttachment {
    aura_terminal::events::ImageAttachment {
        media_type: "image/png".to_string(),
        data: data.to_string(),
        bytes: 3,
    }
}

fn message_with_image(agent_id: AgentId, text: &str, shown: bool) -> UiEvent {
    UiEvent::UserMessage {
        agent_id: agent_id.to_hex(),
        text: text.to_string(),
        shown,
        images: vec![png("aGk=")],
    }
}

/// Media types of the image blocks in the user messages of `agent_id`'s
/// idle session.
fn user_images(state: &LoopState<'_>, agent_id: AgentId) -> Vec<(String, String)> {
    state.idle[&agent_id]
        .messages
        .iter()
        .filter(|m| m.role == aura_model_reasoner::Role::User)
        .flat_map(|m| m.content.iter())
        .filter_map(|block| match block {
            aura_model_reasoner::ContentBlock::Image { source } => {
                Some((source.media_type.clone(), source.data.clone()))
            }
            _ => None,
        })
        .collect()
}

async fn next_done(done: &mut mpsc::Receiver<WorkDone>) -> WorkDone {
    tokio::time::timeout(Duration::from_secs(10), done.recv())
        .await
        .expect("agent work did not finish")
        .expect("work channel closed")
}

/// Commands sent so far, with the agent each was addressed to.
fn drain(commands: &mut mpsc::Receiver<UiCommand>) -> Vec<(Option<String>, UiCommand)> {
    let mut out = Vec::new();
    while let Ok(command) = commands.try_recv() {
        match command {
            UiCommand::ForAgent { agent_id, command } => out.push((Some(agent_id), *command)),
            other => out.push((None, other)),
        }
    }
    out
}

fn completed(commands: &[(Option<String>, UiCommand)], agent_id: AgentId) -> usize {
    let hex = agent_id.to_hex();
    commands
        .iter()
        .filter(|(to, c)| to.as_deref() == Some(hex.as_str()) && matches!(c, UiCommand::Complete))
        .count()
}

#[tokio::test]
async fn another_agent_works_while_the_first_is_busy() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let (first, second) = (fixture.first, fixture.second);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow job", true), &links).await;
    assert!(state.hub.is_running(first));

    handle_ui_event(&mut state, UiEvent::SelectAgent(second.to_hex()), &links).await;
    assert_eq!(state.active, second);
    handle_ui_event(&mut state, message(second, "quick question", true), &links).await;

    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, second);
    handle_work_done(&mut state, finished).await;
    assert!(
        state.hub.is_running(first),
        "the first agent is still working"
    );
    assert_eq!(state.idle[&second].messages.len(), 2);

    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, first);
    handle_work_done(&mut state, finished).await;
    assert!(!state.hub.is_running(first));
    assert_eq!(state.idle[&first].messages.len(), 2);
    assert_eq!(state.in_flight, 0);

    let commands = drain(&mut commands);
    assert_eq!(completed(&commands, first), 1);
    assert_eq!(completed(&commands, second), 1);
    let replies_to_second = commands.iter().any(|(to, c)| {
        to.as_deref() == Some(second.to_hex().as_str())
            && matches!(c, UiCommand::AppendText(_) | UiCommand::ShowMessage(_))
    });
    assert!(
        replies_to_second,
        "the second agent's reply goes to its chat"
    );
}

/// Text of the user messages in `agent_id`'s idle session.
fn user_texts(state: &LoopState<'_>, agent_id: AgentId) -> Vec<String> {
    state.idle[&agent_id]
        .messages
        .iter()
        .filter(|m| m.role == aura_model_reasoner::Role::User)
        .map(aura_model_reasoner::Message::text_content)
        .collect()
}

#[tokio::test]
async fn message_to_a_busy_agent_goes_into_its_turn() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow start", true), &links).await;
    handle_ui_event(&mut state, message(first, "follow-up", false), &links).await;
    assert!(state.queued.is_empty(), "nothing waits for the turn to end");

    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    assert!(finished.unseen.is_empty());
    handle_work_done(&mut state, finished).await;
    assert!(!state.hub.is_running(first), "one turn answered both");

    // Depending on timing the model gets the message with its first call
    // or after it, so only the order is fixed.
    let prompts = user_texts(&state, first).concat();
    let steered = format!("{}follow-up", SteerSource::User.prefix());
    assert!(prompts.starts_with("slow start"), "{prompts:?}");
    assert!(prompts.ends_with(&steered), "{prompts:?}");

    let commands = drain(&mut commands);
    let echoed = commands
        .iter()
        .any(|(_, c)| matches!(c, UiCommand::UserSteered(p) if p == "follow-up"));
    assert!(echoed, "the chat shows the message when the turn takes it");
}

#[tokio::test]
async fn mentioned_files_reach_the_model_but_the_chat_keeps_the_typed_text() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let workspace = state.kernel_for(first).unwrap().agent_workspace();
    std::fs::write(workspace.join("notes.md"), "remember the milk\n").unwrap();

    let typed = "explain @notes.md and @missing.md";
    handle_ui_event(&mut state, message(first, typed, true), &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    let prompts = user_texts(&state, first);
    assert_eq!(
        prompts[0],
        format!(
            "{typed}\n\n<attached_file path=\"notes.md\">\nremember the milk\n</attached_file>"
        )
    );
    let records: Vec<String> = drain(&mut commands)
        .into_iter()
        .filter_map(|(_, c)| match c {
            UiCommand::NewRecord(record) => Some(record.message),
            _ => None,
        })
        .collect();
    assert!(records.iter().any(|m| m.contains(typed)), "{records:?}");
    assert!(
        !records.iter().any(|m| m.contains("remember the milk")),
        "the record keeps the prompt as typed: {records:?}"
    );
}

#[tokio::test]
async fn a_pasted_image_goes_to_the_model_as_an_image_block() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let event = message_with_image(first, "[image 1] what is this?", true);
    handle_ui_event(&mut state, event, &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    let prompt = state.idle[&first]
        .messages
        .iter()
        .find(|m| m.role == aura_model_reasoner::Role::User)
        .unwrap();
    assert!(matches!(
        prompt.content.as_slice(),
        [
            aura_model_reasoner::ContentBlock::Image { .. },
            aura_model_reasoner::ContentBlock::Text { text },
        ] if text == "[image 1] what is this?"
    ));
    assert_eq!(
        user_images(&state, first),
        [("image/png".to_string(), "aGk=".to_string())]
    );
}

#[tokio::test]
async fn an_image_for_a_busy_agent_waits_for_the_next_turn() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow start", true), &links).await;
    let event = message_with_image(first, "[image 1] and this", false);
    handle_ui_event(&mut state, event, &links).await;
    assert_eq!(state.queued[&first].len(), 1, "a turn can't take images");

    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    assert_eq!(
        user_texts(&state, first),
        ["slow start", "[image 1] and this"]
    );
    assert_eq!(user_images(&state, first).len(), 1);
    let shown = drain(&mut commands).into_iter().any(|(_, c)| {
        matches!(c, UiCommand::TurnStarted { prompt: Some(p) } if p == "[image 1] and this")
    });
    assert!(
        shown,
        "the chat shows the queued message when its turn starts"
    );
}

#[tokio::test]
async fn a_harness_notice_never_attaches_files() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let workspace = state.kernel_for(first).unwrap().agent_workspace();
    std::fs::write(workspace.join("notes.md"), "remember the milk\n").unwrap();

    deliver(
        &mut state,
        first,
        SteerSource::Harness,
        "job log mentions @notes.md".to_string(),
        false,
    )
    .await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    let prompts = user_texts(&state, first);
    assert!(!prompts[0].contains("remember the milk"), "{prompts:?}");
}

fn job_finished(agent_id: AgentId, killed: bool) -> JobFinished {
    JobFinished {
        job_id: "job-1".to_string(),
        agent_id: Some(agent_id),
        command: "cargo test".to_string(),
        exit_code: Some(0),
        killed,
        elapsed: Duration::from_secs(3),
        tail: "test result: ok".to_string(),
    }
}

#[tokio::test]
async fn finished_job_starts_a_turn_for_an_idle_agent() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_job_finished(&mut state, job_finished(first, false)).await;
    assert!(state.hub.is_running(first));

    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let prompts = user_texts(&state, first);
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].starts_with(SteerSource::Harness.prefix()));
    assert!(prompts[0].contains("job-1") && prompts[0].contains("test result: ok"));
}

#[tokio::test]
async fn stopped_job_needs_no_reply() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);

    handle_job_finished(&mut state, job_finished(first, true)).await;

    assert!(!state.hub.is_running(first));
    assert!(state.queued.is_empty());
}

#[tokio::test]
async fn messages_the_turn_never_saw_run_next() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let session = state.idle.remove(&first).unwrap();

    handle_work_done(
        &mut state,
        WorkDone {
            agent_id: first,
            session: Some(session),
            failure: None,
            unseen: vec![(SteerSource::User, "late".to_string())],
        },
    )
    .await;
    assert!(state.hub.is_running(first), "the late message started");

    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(user_texts(&state, first), ["late"]);
}

#[tokio::test]
async fn cancel_stops_only_the_named_agent() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let (first, second) = (fixture.first, fixture.second);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow one", true), &links).await;
    handle_ui_event(&mut state, message(second, "slow two", true), &links).await;
    handle_ui_event(&mut state, UiEvent::Cancel(first.to_hex()), &links).await;

    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, first);
    handle_work_done(&mut state, finished).await;
    assert!(state.hub.is_running(second));

    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, second);
    handle_work_done(&mut state, finished).await;
    let _ = drain(&mut commands);
}

#[tokio::test]
async fn switching_to_a_busy_agent_keeps_its_live_chat() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let (first, second) = (fixture.first, fixture.second);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow job", true), &links).await;
    handle_ui_event(&mut state, UiEvent::SelectAgent(second.to_hex()), &links).await;
    let _ = drain(&mut commands);

    handle_ui_event(&mut state, UiEvent::SelectAgent(first.to_hex()), &links).await;
    let commands = drain(&mut commands);
    assert!(matches!(
        commands.first(),
        Some((None, UiCommand::SetActiveAgent(id))) if *id == first.to_hex()
    ));
    let reloaded = commands
        .iter()
        .any(|(_, c)| matches!(c, UiCommand::LoadConversation(_)));
    assert!(!reloaded, "a busy agent's chat is live, not reloaded");

    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
}

#[tokio::test]
async fn compact_summarizes_the_older_history() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let history: Vec<aura_model_reasoner::Message> =
        std::iter::once(aura_model_reasoner::Message::user("build it"))
            .chain((0..12).flat_map(|i| {
                [
                    aura_model_reasoner::Message::assistant(format!(
                        "step {i}: {}",
                        "x".repeat(400)
                    )),
                    aura_model_reasoner::Message::user(format!("ok {i}")),
                ]
            }))
            .collect();
    let before = history.len();
    state.idle.get_mut(&first).unwrap().messages = history;

    let compact = UiEvent::Compact {
        agent_id: first.to_hex(),
        focus: Some("the step numbers".to_string()),
    };
    handle_ui_event(&mut state, compact, &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    let messages = &state.idle[&first].messages;
    assert!(messages.len() < before, "{} messages left", messages.len());
    assert_eq!(messages[0].text_content(), "build it");
    let commands = drain(&mut commands);
    assert!(commands
        .iter()
        .any(|(_, c)| matches!(c, UiCommand::Compacted(_))));
    assert_eq!(completed(&commands, first), 1);
}

#[tokio::test]
async fn queued_work_is_counted_for_the_status_line() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow job", true), &links).await;
    let command = UiEvent::RunCommand {
        agent_id: first.to_hex(),
        command: "true".to_string(),
        cols: 80,
    };
    handle_ui_event(&mut state, command, &links).await;
    let queued: Vec<usize> = drain(&mut commands)
        .into_iter()
        .filter_map(|(_, c)| match c {
            UiCommand::SetQueued(n) => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(queued, [1]);

    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let queued: Vec<usize> = drain(&mut commands)
        .into_iter()
        .filter_map(|(_, c)| match c {
            UiCommand::SetQueued(n) => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(queued, [0]);
}

/// A tool call with its result, as a turn leaves them in the history.
fn tool_exchange(id: &str) -> [aura_model_reasoner::Message; 2] {
    use aura_model_reasoner::{ContentBlock, Message, Role, ToolResultContent};
    [
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({ "path": "src/main.rs" }),
            }],
        ),
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: ToolResultContent::Text("fn main() {}".to_string()),
                is_error: false,
            }],
        ),
    ]
}

fn open_again(fixture: &Fixture, agent_id: AgentId) -> AgentSession {
    let agent = lock_registry_agent(fixture, agent_id);
    fixture.hub.factory.open(&agent, Vec::new(), None).unwrap()
}

fn todo(content: &str) -> aura_tools::todo::TodoItem {
    aura_tools::todo::TodoItem {
        id: "1".to_string(),
        content: content.to_string(),
        status: aura_tools::todo::TodoStatus::InProgress,
    }
}

#[tokio::test]
async fn a_restart_keeps_tool_calls_todos_and_usage() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "read main", true), &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let session = state.idle.get_mut(&first).unwrap();
    session.messages.extend(tool_exchange("t1"));
    session.todos = vec![todo("Fix main")];
    session.usage.calls = 4;
    session.save_checkpoint().await;

    let reopened = open_again(&fixture, first);
    assert_eq!(reopened.messages.len(), 4, "{:?}", reopened.messages);
    assert!(reopened.messages[2]
        .content
        .iter()
        .any(|b| matches!(b, aura_model_reasoner::ContentBlock::ToolUse { .. })));
    assert_eq!(reopened.todos, [todo("Fix main")]);
    assert_eq!(reopened.usage.calls, 4);
    assert_eq!(reopened.started_ms, state.idle[&first].started_ms);
}

#[tokio::test]
async fn the_record_past_a_checkpoint_is_replayed() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let session = state.idle.get_mut(&first).unwrap();
    session.messages = tool_exchange("t1").to_vec();
    session.save_checkpoint().await;

    let later = Transaction::user_prompt(first, "written after the checkpoint");
    session.kernel.process_direct(later).await.unwrap();

    let reopened = open_again(&fixture, first);
    assert_eq!(reopened.messages.len(), 3);
    assert_eq!(
        reopened.messages[2].text_content(),
        "written after the checkpoint"
    );
}

#[tokio::test]
async fn a_new_session_drops_the_checkpoint_and_resume_brings_it_back() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let session = state.idle.get_mut(&first).unwrap();
    session.messages = vec![aura_model_reasoner::Message::user("old task")];
    session.messages.extend(tool_exchange("t1"));
    session.todos = vec![todo("Finish the old task")];
    session.started_ms = 1_000;
    session.save_checkpoint().await;

    assert!(handlers::handle_new_session(&mut state).await);
    assert!(open_again(&fixture, first).messages.is_empty());

    handlers::handle_resume(&mut state, first, None).await;
    let listed = drain(&mut commands).into_iter().any(|(_, c)| {
        matches!(c, UiCommand::ShowMessage(m) if m.content.contains("1. ") && m.content.contains("old task"))
    });
    assert!(listed, "the old session is listed");

    handlers::handle_resume(&mut state, first, Some(1)).await;
    assert_eq!(state.idle[&first].messages.len(), 3);
    assert_eq!(state.idle[&first].todos, [todo("Finish the old task")]);
    let commands = drain(&mut commands);
    assert!(commands
        .iter()
        .any(|(_, c)| matches!(c, UiCommand::LoadConversation(chat) if chat.len() == 2)));

    let reopened = open_again(&fixture, first);
    assert_eq!(reopened.messages.len(), 3, "the resumed session is current");
    assert_eq!(reopened.started_ms, 1_000);
}

/// Wait for the approval prompt addressed to `agent_id` and return its id.
async fn approval_for(commands: &mut mpsc::Receiver<UiCommand>, agent_id: AgentId) -> String {
    let hex = agent_id.to_hex();
    let wait = async {
        loop {
            match commands.recv().await.expect("command channel closed") {
                UiCommand::ForAgent { agent_id, command } if agent_id == hex => {
                    if let UiCommand::RequestApproval { id, .. } = *command {
                        return id;
                    }
                }
                _ => {}
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .expect("no approval was asked for")
}

/// Results of the tool calls in `agent_id`'s idle session, as `is_error`.
fn tool_results(state: &LoopState<'_>, agent_id: AgentId) -> Vec<bool> {
    state.idle[&agent_id]
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            aura_model_reasoner::ContentBlock::ToolResult { is_error, .. } => Some(*is_error),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_command_waits_for_approval_and_runs_once_allowed() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "run ls", true), &links).await;
    let id = approval_for(&mut commands, first).await;
    assert!(state.hub.is_running(first));
    assert!(done.try_recv().is_err(), "the turn waits for the answer");

    handle_ui_event(&mut state, UiEvent::Approve(id), &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(tool_results(&state, first), [false]);
}

#[tokio::test]
async fn cancel_denies_the_waiting_call_and_ends_the_turn() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "run ls", true), &links).await;
    let id = approval_for(&mut commands, first).await;
    handle_ui_event(&mut state, UiEvent::Cancel(first.to_hex()), &links).await;

    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert!(!state.approvals.is_waiting(first));
    assert!(!state.hub.is_running(first));
    let closed = drain(&mut commands)
        .into_iter()
        .any(|(_, c)| matches!(c, UiCommand::CloseApproval(ref c) if *c == id));
    assert!(closed, "the prompt is taken down");
}

#[tokio::test]
async fn a_background_agent_asks_in_its_own_chat() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let (first, second) = (fixture.first, fixture.second);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    assert_eq!(state.active, first);

    handle_ui_event(&mut state, message(second, "run ls", true), &links).await;
    let id = approval_for(&mut commands, second).await;
    assert!(id.starts_with(&second.to_hex()), "{id}");

    handle_ui_event(&mut state, UiEvent::Deny(id), &links).await;
    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, second);
    handle_work_done(&mut state, finished).await;
    assert_eq!(tool_results(&state, second), [true], "the call was denied");
}

#[tokio::test]
async fn a_mode_switch_is_recorded_before_it_takes_effect() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let set_plan = UiEvent::SetMode {
        agent_id: first.to_hex(),
        mode: PermissionMode::Plan,
    };

    handle_ui_event(&mut state, set_plan, &links).await;
    let kernel = Arc::clone(&state.idle[&first].kernel);
    assert_eq!(kernel.policy().mode_label(), "plan");
    assert_eq!(state.hub.mode_of(first), PermissionMode::Plan);
    let records = kernel.store().scan_record(first, 0, 100).unwrap();
    let switch = records
        .iter()
        .find(|e| {
            e.tx.tx_type == aura_core_types::TransactionType::System
                && String::from_utf8_lossy(&e.tx.payload).contains("permission_mode")
        })
        .expect("a System record for the switch");
    let payload: serde_json::Value = serde_json::from_slice(&switch.tx.payload).unwrap();
    assert_eq!(payload["from"], "default");
    assert_eq!(payload["to"], "plan");

    handle_ui_event(&mut state, UiEvent::CycleMode(first.to_hex()), &links).await;
    assert_eq!(state.hub.mode_of(first), PermissionMode::Unrestricted);
    let shown: Vec<PermissionMode> = drain(&mut commands)
        .into_iter()
        .filter_map(|(to, c)| match c {
            UiCommand::SetMode(mode) if to == Some(first.to_hex()) => Some(mode),
            _ => None,
        })
        .collect();
    assert_eq!(shown, [PermissionMode::Plan, PermissionMode::Unrestricted]);
}

#[tokio::test]
async fn unrestricted_mode_runs_commands_without_asking() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let unrestricted = UiEvent::SetMode {
        agent_id: first.to_hex(),
        mode: PermissionMode::Unrestricted,
    };

    handle_ui_event(&mut state, unrestricted, &links).await;
    handle_ui_event(&mut state, message(first, "run ls", true), &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(tool_results(&state, first), [false]);
    let asked = drain(&mut commands)
        .into_iter()
        .any(|(_, c)| matches!(c, UiCommand::RequestApproval { .. }));
    assert!(!asked);
}

#[tokio::test]
async fn init_asks_the_agent_to_improve_an_existing_agents_md() {
    let (fixture, mut commands) = fixture();
    std::fs::write(fixture._dirs[2].path().join("AGENTS.md"), "Old notes.").unwrap();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let init = UiEvent::InitProject {
        agent_id: first.to_hex(),
    };
    handle_ui_event(&mut state, init, &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    let prompts = user_texts(&state, first);
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains("already has an AGENTS.md"),
        "{prompts:?}"
    );
    let commands = drain(&mut commands);
    assert!(
        commands.iter().any(|(_, c)| matches!(
            c,
            UiCommand::TurnStarted { prompt: Some(p) } if p.contains("improve it")
        )),
        "the chat shows the request"
    );
    assert!(
        commands.iter().any(|(_, c)| matches!(
            c,
            UiCommand::SetInstructions(files)
                if files.iter().any(|f| f.path.ends_with("AGENTS.md"))
        )),
        "the agent's chat learns which instruction files it loads"
    );
}

/// Run a turn of `agent_id` the way `run_prompt` checkpoints it: the
/// prompt and reply go into the session, and each `(file, content)` is
/// written through the file hook (`None` deletes the file).
async fn checkpointed_turn(
    fixture: &Fixture,
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    prompt: &str,
    edits: &[(&str, Option<&str>)],
) {
    use aura_model_reasoner::Message;

    let session = state.idle.get_mut(&agent_id).unwrap();
    fixture
        .files
        .begin_turn(agent_id, session.started_ms, session.messages.len(), prompt);
    session.messages.push(Message::user(prompt));
    let hook = fixture.files.hook(vec![agent_id], None);
    for (name, content) in edits {
        let path = fixture.workspace.join(name);
        hook.before_change(&path, Some("t")).await;
        match content {
            Some(text) => std::fs::write(&path, text).unwrap(),
            None => std::fs::remove_file(&path).unwrap(),
        }
        hook.after_change(&path, Some("t"), true).await;
    }
    session
        .messages
        .push(Message::assistant(format!("did {prompt}")));
    fixture.files.end_turn(agent_id);
}

fn system_texts(commands: &[(Option<String>, UiCommand)]) -> String {
    commands
        .iter()
        .filter_map(|(_, c)| match c {
            UiCommand::ShowMessage(m) => Some(m.content.clone()),
            UiCommand::ShowSuccess(s) | UiCommand::ShowWarning(s) => Some(s.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn rewind_restores_files_and_the_conversation() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let notes = fixture.workspace.join("notes.txt");
    std::fs::write(&notes, "original\n").unwrap();

    checkpointed_turn(
        &fixture,
        &mut state,
        first,
        "one",
        &[("notes.txt", Some("one\n"))],
    )
    .await;
    checkpointed_turn(
        &fixture,
        &mut state,
        first,
        "two",
        &[("notes.txt", Some("two\n")), ("added.txt", Some("new\n"))],
    )
    .await;
    let head_before = state.idle[&first]
        .kernel
        .store()
        .get_head_seq(first)
        .unwrap();

    rewind::handle_rewind(&mut state, first, None, RewindScope::Both).await;
    let listed = system_texts(&drain(&mut commands));
    assert!(
        listed.contains("2. ") && listed.contains("two · 2 files"),
        "{listed}"
    );
    assert!(
        listed.contains("1. ") && listed.contains("one · 1 file"),
        "{listed}"
    );
    assert!(listed.contains("not undone"), "{listed}");

    rewind::handle_rewind(&mut state, first, Some(2), RewindScope::Both).await;
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "one\n");
    assert!(!fixture.workspace.join("added.txt").exists());
    assert_eq!(user_texts(&state, first), ["one"]);
    assert_eq!(state.idle[&first].messages.len(), 2);

    let commands_after = drain(&mut commands);
    assert!(commands_after
        .iter()
        .any(|(_, c)| matches!(c, UiCommand::LoadConversation(chat) if chat.len() == 2)));
    let said = system_texts(&commands_after);
    assert!(
        said.contains("Rewound to before turn 2: restored 2 files and the conversation"),
        "{said}"
    );
    let head_after = state.idle[&first]
        .kernel
        .store()
        .get_head_seq(first)
        .unwrap();
    assert!(head_after > head_before, "the rewind is recorded");

    let reopened = open_again(&fixture, first);
    assert_eq!(reopened.messages.len(), 2, "the checkpoint holds the cut");
    assert_eq!(fixture.files.turns(first, reopened.started_ms).len(), 1);
}

#[tokio::test]
async fn rewinding_only_code_keeps_the_conversation() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let file = fixture.workspace.join("a.txt");
    std::fs::write(&file, "before\n").unwrap();

    checkpointed_turn(&fixture, &mut state, first, "edit", &[("a.txt", None)]).await;
    rewind::handle_rewind(&mut state, first, Some(1), RewindScope::Code).await;

    assert_eq!(std::fs::read_to_string(&file).unwrap(), "before\n");
    assert_eq!(state.idle[&first].messages.len(), 2);
    assert_eq!(
        fixture
            .files
            .turns(first, state.idle[&first].started_ms)
            .len(),
        1
    );
}

#[tokio::test]
async fn a_summarized_turn_cannot_be_rewound_in_the_chat() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let file = fixture.workspace.join("a.txt");
    std::fs::write(&file, "before\n").unwrap();

    checkpointed_turn(
        &fixture,
        &mut state,
        first,
        "edit",
        &[("a.txt", Some("x\n"))],
    )
    .await;
    state.idle.get_mut(&first).unwrap().messages = vec![aura_model_reasoner::Message::user(
        "summary of earlier work",
    )];
    rewind::handle_rewind(&mut state, first, Some(1), RewindScope::Both).await;

    let said = system_texts(&drain(&mut commands));
    assert!(said.contains("/rewind 1 code"), "{said}");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "x\n",
        "files stay when the chat can't follow"
    );
}

#[tokio::test]
async fn rewinding_a_busy_agent_waits() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let session = state.idle.remove(&first).unwrap();

    rewind::handle_rewind(&mut state, first, Some(1), RewindScope::Both).await;
    let said = system_texts(&drain(&mut commands));
    assert!(said.contains("Wait for this agent"), "{said}");
    state.idle.insert(first, session);
}

#[tokio::test]
async fn diff_shows_the_latest_turn_or_the_whole_session() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    std::fs::write(fixture.workspace.join("a.txt"), "a\n").unwrap();

    rewind::handle_diff(&state, first, false).await;
    assert!(system_texts(&drain(&mut commands)).contains("No turns"));

    checkpointed_turn(
        &fixture,
        &mut state,
        first,
        "one",
        &[("a.txt", Some("A\n"))],
    )
    .await;
    checkpointed_turn(
        &fixture,
        &mut state,
        first,
        "two",
        &[("b.txt", Some("b\n"))],
    )
    .await;

    let shown = |commands: Vec<(Option<String>, UiCommand)>| {
        commands.into_iter().find_map(|(_, c)| match c {
            UiCommand::ShowDiff { title, files } => Some((title, files)),
            _ => None,
        })
    };
    rewind::handle_diff(&state, first, false).await;
    let (title, files) = shown(drain(&mut commands)).expect("a diff");
    assert_eq!(title, "Files changed by turn 2 · two");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "b.txt");
    assert!(files[0].diff.contains("+b"), "{}", files[0].diff);

    rewind::handle_diff(&state, first, true).await;
    let (_, files) = shown(drain(&mut commands)).expect("a diff");
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["a.txt", "b.txt"]);
    assert!(files[0].diff.contains("-a\n+A\n"), "{}", files[0].diff);
}
