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
    MockProvider, ModelProvider, ModelRequest, ModelResponse, ReasonerError,
};
use aura_store_db::RocksStore;
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
        let slow = request
            .messages
            .last()
            .is_some_and(|m| m.text_content().contains("slow"));
        if slow {
            self.gate.acquire().await.unwrap().forget();
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
    first: AgentId,
    second: AgentId,
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
    };
    let gate = Arc::new(Semaphore::new(0));
    let provider_gate = Arc::clone(&gate);
    let workspace_root = workspace.path().to_path_buf();
    let hub = AgentHub::new(parts, |_dispatcher| {
        let kernel_store = Arc::clone(&store);
        KernelFactory::new(
            store,
            Box::new(move |agent_id, _parent_chain, folder| {
                let provider: Arc<dyn ModelProvider + Send + Sync> = Arc::new(GatedProvider {
                    inner: MockProvider::simple_response("Done."),
                    gate: Arc::clone(&provider_gate),
                });
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
    let fixture = Fixture {
        hub,
        config,
        gate,
        commands_tx,
        first,
        second,
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
    }
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
