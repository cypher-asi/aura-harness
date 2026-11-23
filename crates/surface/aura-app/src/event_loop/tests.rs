use super::*;
use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::record_loader::json_payload;
use crate::session_helpers::{build_executor_router_with_config, default_agent_config_with_auth};
use async_trait::async_trait;
use aura_agent::AgentLoopConfig;
use aura_agent_kernel::KernelConfig;
use aura_auth::{Vault, ZosIdentity};
use aura_context_skills::{SkillLoader, SkillManager};
use aura_core_types::{
    Hash, OutcomePayload, OutcomeSignal, PromptCategory, PromptPayload, Route, RouteLevel,
};
use aura_exec_tools::ToolConfig;
use aura_gate::live::GateLive;
use aura_model::{
    MockProvider, MockResponse, ModelProvider, ModelRequest, ModelResponse, ReasonerError,
};
use aura_store_db::RocksStore;
use aura_ui_terminal::events::{PolicyView, RewindScope, ToolPolicyState};
use aura_ui_terminal::{ApprovalLevel, PermissionMode};
use tempfile::TempDir;
use tokio::sync::{Barrier, Semaphore};

mod compare_runs;
mod delegation;
mod escalation;
mod flow;
mod queue_edits;
mod subtasks;

/// Model whose route refuses the sign-in, for Gate's re-routing.
const REFUSING_MODEL: &str = "claude/claude-sonnet-4-6";

/// Answers like a mock model, but holds turn calls whose last message
/// mentions "slow" until the test releases them, and refuses
/// [`REFUSING_MODEL`]. Auxiliary calls, like the chat title, quote the
/// prompt and are answered at once. Gate's judge of failed work blames
/// the model when the work's request says "judge: blame", excuses it
/// when it says "judge: excuse", and gives no verdict otherwise. A turn
/// asked to "reply unverified" ends on the fast tier saying its work
/// isn't verified, and Gate's judge of unverified work blames such a
/// reply when the request says "review: blame", finds it fine when it
/// says "review: fine", and gives no verdict otherwise. Gate's review of
/// work about to stand finds it unfinished when the request says
/// "inspect: blame", finds it unfinished until the model was sent back
/// to it when it says "inspect: once", finds it done when it says
/// "inspect: fine", and gives no verdict otherwise; a model sent back to
/// its work replies that it fixed what the review found. The
/// comparison judge prefers the first attempt, and, given what the user
/// kept, rates it as good as that and the second worse; of work "judged
/// together", it answers once a second judge is asked at the same time,
/// and fails after [`JUDGED_TOGETHER`] without one.
struct GatedProvider {
    inner: MockProvider,
    gate: Arc<Semaphore>,
    judges: Arc<Barrier>,
}

/// How long the comparison judge of work "judged together" waits for a
/// second judge.
const JUDGED_TOGETHER: Duration = Duration::from_secs(5);

/// What a model sent back to its work by Gate's review replies.
const REPAIRED_REPLY: &str = "Fixed what the review found.";

#[async_trait]
impl ModelProvider for GatedProvider {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        if request.model.as_str() == REFUSING_MODEL {
            return Err(ReasonerError::Api {
                status: 401,
                message: "claude plan models are not available in this session".to_string(),
            });
        }
        let last = request.messages.last().map(|m| m.text_content());
        if request.system == aura_gate::judge::classify::SYSTEM_PROMPT {
            return classify(&request, last.as_deref().unwrap_or_default()).await;
        }
        if request.system == aura_gate::judge::criteria::SYSTEM_PROMPT
            && last
                .as_deref()
                .is_some_and(|text| text.contains("criteria writer down"))
        {
            return Err(ReasonerError::Api {
                status: 400,
                message: "tool_choice: type \"tool\" and \"any\" are not supported for this model."
                    .to_string(),
            });
        }
        if request.system == aura_gate::judge::criteria::SYSTEM_PROMPT {
            let asked = last.as_deref().unwrap_or_default();
            let kind = if asked.contains("(exacting criteria)") {
                Some("exact_output")
            } else if asked.contains("(plain criteria)") {
                Some("behaviour")
            } else {
                None
            };
            if let Some(kind) = kind {
                let input = serde_json::json!({
                    "items": [{"text": "prints the parse tree", "kind": kind}],
                    "strict": kind == "exact_output",
                });
                return MockProvider::new()
                    .with_response(MockResponse::tool_use(
                        "toolu_criteria",
                        aura_gate::judge::criteria::CRITERIA_TOOL,
                        input,
                    ))
                    .complete(request)
                    .await;
            }
        }
        if request.system == aura_gate::judge::turn::SYSTEM_PROMPT {
            let asked = last.as_deref().unwrap_or_default();
            let reply = if asked.contains("judge: blame") {
                "The change is wrong.\nVERDICT: MODEL"
            } else if asked.contains("judge: excuse") {
                "The check cannot pass here.\nVERDICT: OTHER"
            } else {
                "Hard to say."
            };
            return MockProvider::simple_response(reply).complete(request).await;
        }
        if request.system == aura_gate::judge::turn::UNVERIFIED_SYSTEM_PROMPT {
            let asked = last.as_deref().unwrap_or_default();
            let disowned = asked.contains("it isn't verified");
            let reply = if disowned && asked.contains("review: blame") {
                "The change is untested and wrong.\nVERDICT: MODEL"
            } else if disowned && asked.contains("review: fine") {
                "The change is right.\nVERDICT: OK"
            } else {
                "Hard to say."
            };
            return MockProvider::simple_response(reply).complete(request).await;
        }
        if request.system == aura_gate::judge::turn::REVIEW_SYSTEM_PROMPT {
            let asked = last.as_deref().unwrap_or_default();
            let repaired = asked.contains(REPAIRED_REPLY);
            let once = asked.contains("inspect: once");
            let reply = if asked.contains("inspect: blame") || (once && !repaired) {
                "1. The fix misses the empty case.\nVERDICT: MODEL"
            } else if once || asked.contains("inspect: fine") {
                "Every requirement is met.\nVERDICT: OK"
            } else {
                "Hard to say."
            };
            return MockProvider::simple_response(reply).complete(request).await;
        }
        if request.system == aura_gate::judge::blind::SYSTEM_PROMPT {
            let asked = last.as_deref().unwrap_or_default();
            if asked.contains("judged together")
                && tokio::time::timeout(JUDGED_TOGETHER, self.judges.wait())
                    .await
                    .is_err()
            {
                return Err(ReasonerError::Api {
                    status: 408,
                    message: "no other pair was being judged".to_string(),
                });
            }
            let reply = if asked.contains("## Reference") {
                "A is tidier.\nA_VS_REFERENCE: AS_GOOD\nB_VS_REFERENCE: WORSE\nWINNER: A"
            } else {
                "A is tidier.\nWINNER: A"
            };
            return MockProvider::simple_response(reply).complete(request).await;
        }
        let runs_check = last.as_deref().is_some_and(|text| {
            text.contains("run failing check") || text.contains("run passing check")
        });
        if !runs_check
            && request
                .messages
                .iter()
                .any(|m| m.text_content().contains("write the answer"))
        {
            return answer(request, last.as_deref()).await;
        }
        let auxiliary = request.metadata.kind == Some(aura_model::ModelRequestKind::Auxiliary);
        let slow = !auxiliary && last.as_deref().is_some_and(|text| text.contains("slow"));
        if slow {
            self.gate.acquire().await.unwrap().forget();
        }
        if let Some(response) = scripted(last.as_deref().unwrap_or_default(), &request) {
            let model = request.model.as_str().to_string();
            let mut response = MockProvider::new()
                .with_response(response)
                .complete(request)
                .await?;
            response.trace.model.clone_from(&model);
            response.model_used = model;
            return Ok(response);
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
        let check = |args: &[&str]| {
            MockResponse::tool_use(
                "toolu_check",
                "run_command",
                serde_json::json!({ "program": "cargo", "args": args }),
            )
        };
        if last
            .as_deref()
            .is_some_and(|text| text.contains("run passing check"))
        {
            return MockProvider::new()
                .with_response(check(&["check", "--help"]))
                .complete(request)
                .await;
        }
        if last
            .as_deref()
            .is_some_and(|text| text.contains("run failing check"))
        {
            return MockProvider::new()
                .with_response(check(&["test", "--no-such-flag"]))
                .complete(request)
                .await;
        }
        if last.as_deref().is_some_and(|text| text.contains("explode")) {
            return Err(ReasonerError::Api {
                status: 400,
                message: "the model fell over".to_string(),
            });
        }
        let disowns = !auxiliary
            && request.model.as_str().contains("haiku")
            && request
                .messages
                .iter()
                .any(|m| m.text_content().contains("reply unverified"));
        if disowns {
            return MockProvider::simple_response("I made the change, but it isn't verified.")
                .complete(request)
                .await;
        }
        let sent_back = !auxiliary
            && last
                .as_deref()
                .is_some_and(|text| text.contains(aura_gate::followup::REPAIR_OPENING));
        if sent_back {
            return MockProvider::simple_response(REPAIRED_REPLY)
                .complete(request)
                .await;
        }
        if auxiliary {
            // The chat title is written in the background while the turn
            // runs, racing it on the same model. `inner`'s queue holds the
            // one scripted answer for the turn itself, so an auxiliary
            // call like this must not draw from it.
            return MockProvider::simple_response("Mock title")
                .complete(request)
                .await;
        }
        self.inner.complete(request).await
    }

    async fn health_check(&self) -> bool {
        true
    }
}

/// The one tool call a turn whose last message is `last` makes, for the
/// delegation tests: "write a note" writes `notes/<model>.txt`, "touch a
/// file" runs `touch touched.txt`, "read the big file" reads `big.txt`,
/// and "change files by shell" appends the model to `tracked.txt` and
/// `target/out` and writes `shell-<model>.txt` with a shell command, which
/// on a first attempt also deletes `gone.txt`.
fn scripted(last: &str, request: &ModelRequest) -> Option<MockResponse> {
    let call = |name: &str, input: serde_json::Value| {
        Some(MockResponse::tool_use(format!("toolu_{name}"), name, input))
    };
    if last.contains("write a note") {
        let path = format!("notes/{}.txt", request.model.as_str());
        return call(
            "write_file",
            serde_json::json!({ "path": path, "content": "note" }),
        );
    }
    if last.contains("touch a file") {
        return call(
            "run_command",
            serde_json::json!({ "program": "touch", "args": ["touched.txt"] }),
        );
    }
    if last.contains("read the big file") {
        return call("read_file", serde_json::json!({ "path": "big.txt" }));
    }
    if last.contains("change files by shell") {
        let model = request.model.as_str();
        let mut script = format!(
            "echo {model} >> tracked.txt && echo {model} > shell-{model}.txt \
             && echo {model} >> target/out"
        );
        if !last.contains("A first attempt") {
            script.push_str(" && rm gone.txt");
        }
        return call(
            "run_command",
            serde_json::json!({ "program": "sh", "args": ["-c", script] }),
        );
    }
    None
}

/// A turn told to "write the answer": it writes `answer.txt` naming the
/// model that answered, then says it is done. Calls are billed as the
/// model asked for, so they have a list price.
async fn answer(request: ModelRequest, last: Option<&str>) -> Result<ModelResponse, ReasonerError> {
    let model = request.model.as_str().to_string();
    let provider = if last.is_some_and(|text| text.contains("write the answer")) {
        MockProvider::new().with_response(MockResponse::tool_use(
            "toolu_answer",
            "write_file",
            serde_json::json!({ "path": "answer.txt", "content": model }),
        ))
    } else {
        MockProvider::simple_response(format!("Wrote the answer on {model}."))
    };
    let mut response = provider.complete(request).await?;
    response.trace.model.clone_from(&model);
    response.model_used = model;
    Ok(response)
}

/// The classifier's answer to `request`, asking about `prompt`: prompts
/// that say "across files" are changes across several files, those that
/// say "plan:" are planning, those that say "fix" are debugging,
/// "classifier down" fails the call, "classifier rambles" gets a reply
/// with no label, and the rest are questions. Prompts that say "(easy)"
/// or "(hard)" get that difficulty; the rest are medium. The answer is a
/// call to the label tool when the request offers it, else text.
async fn classify(request: &ModelRequest, prompt: &str) -> Result<ModelResponse, ReasonerError> {
    if prompt.contains("classifier down") {
        return Err(ReasonerError::Internal("classifier offline".to_string()));
    }
    if prompt.contains("classifier rambles") {
        return MockProvider::simple_response("I'll start by reading the code.")
            .complete(request.clone())
            .await;
    }
    let label = if prompt.contains("across files") {
        "multi_file_change"
    } else if prompt.contains("plan:") {
        "planning"
    } else if prompt.contains("fix") {
        "debugging"
    } else {
        "question"
    };
    let difficulty = ["easy", "hard"]
        .into_iter()
        .find(|d| prompt.contains(&format!("({d})")))
        .unwrap_or("medium");
    let tool = aura_gate::judge::classify::LABEL_TOOL;
    let answer = if request.tools.iter().any(|t| t.name == tool) {
        MockResponse::tool_use(
            "toolu_label",
            tool,
            serde_json::json!({"category": label, "difficulty": difficulty}),
        )
    } else {
        MockResponse::text(format!("{label} {difficulty}"))
    };
    MockProvider::new()
        .with_response(answer)
        .complete(request.clone())
        .await
}

struct Fixture {
    hub: Arc<AgentHub>,
    /// Runs the `task` calls of the hub's kernels
    dispatcher: Arc<dyn aura_exec_tools::SubagentDispatchHook>,
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

/// Gate settings for tests: the defaults, without exploring.
fn steady_gate() -> GateLive {
    GateLive::new(aura_config::GateConfig::defaults().without_exploring())
}

/// [`steady_gate`] whose lead keeps its own file tools and commands, as
/// with `AURA_GATE_DELEGATE=off`.
fn steady_gate_without_delegation() -> GateLive {
    GateLive::new(aura_config::GateConfig {
        delegate: false,
        ..aura_config::GateConfig::defaults().without_exploring()
    })
}

fn fixture() -> (Fixture, mpsc::Receiver<UiCommand>) {
    fixture_with_gate(steady_gate())
}

/// [`fixture`] whose kernels feed `gate`.
fn fixture_with_gate(gate: GateLive) -> (Fixture, mpsc::Receiver<UiCommand>) {
    let gate = Arc::new(gate);
    let data = TempDir::new().unwrap();
    let db = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let store = Arc::new(RocksStore::open(db.path(), false).unwrap());
    let (_, tools) = build_executor_router_with_config(&ToolConfig::for_autonomous_dev_loop());
    let config = default_agent_config_with_auth("mock-model", None);
    let (commands_tx, commands) = mpsc::channel(4096);
    let mut registry = AgentRegistry::load_or_init_in(data.path(), workspace.path()).unwrap();
    let first = registry.active().agent_id();
    let second = registry
        .create(Some("Second".to_string()))
        .unwrap()
        .agent_id();
    let approvals = Arc::new(TerminalApprovals::new(commands_tx.clone(), data.path()));
    let traces = Traces::new(
        Some(data.path().join("home")),
        Some(Arc::new(aura_store_db::TraceStore::for_store(&store))),
        None,
        commands_tx.clone(),
    );
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
            gate: GateBase::default(),
        },
        commands: commands_tx.clone(),
        long_run: crate::long_run::LongRunSettings::default(),
        mode: PermissionMode::Agent,
        approval: aura_ui_terminal::ApprovalLevel::Ask,
        approvals: Some(Arc::clone(&approvals)),
        instructions: crate::project_instructions::ProjectInstructions::only_in(workspace.path()),
        processes: ProcessTable::without_os_scan(None, None),
        agents: None,
        record_panel: RecordPanelAgent::default(),
        gate: Arc::clone(&gate),
        traces,
    };
    let record_observer: Arc<dyn aura_agent_kernel::RecordObserver> =
        Arc::new(FanoutObserver(vec![
            Arc::new(UiRecordObserver::new(
                commands_tx.clone(),
                RecordPanelAgent::default(),
            )) as Arc<dyn aura_agent_kernel::RecordObserver>,
            gate as Arc<dyn aura_agent_kernel::RecordObserver>,
        ]));
    let gate = Arc::new(Semaphore::new(0));
    let provider_gate = Arc::clone(&gate);
    let judges = Arc::new(Barrier::new(2));
    let workspace_root = workspace.path().to_path_buf();
    let checkpoints = crate::session_checkpoint::Checkpoints::new(data.path());
    let prompter = Arc::clone(&approvals);
    let files = crate::file_checkpoints::FileCheckpoints::new(data.path());
    let factory_files = files.clone();
    let hook_files = files.clone();
    let mut captured = None;
    let hub = AgentHub::new(parts, |dispatcher| {
        captured = Some(dispatcher);
        let kernel_store = Arc::clone(&store);
        KernelFactory::new(
            store,
            Box::new(move |agent_id, parent_chain, folder| {
                let provider: Arc<dyn ModelProvider + Send + Sync> = Arc::new(GatedProvider {
                    inner: MockProvider::simple_response("Done."),
                    gate: Arc::clone(&provider_gate),
                    judges: Arc::clone(&judges),
                });
                let workspace_base = folder.unwrap_or_else(|| workspace_root.clone());
                prompter.set_folder(agent_id, workspace_base.clone());
                let chain = std::iter::once(agent_id).chain(parent_chain).collect();
                let executor =
                    aura_exec_tools::ToolExecutor::new(ToolConfig::for_autonomous_dev_loop())
                        .with_file_change_hook(hook_files.hook(chain, None));
                let mut executors = aura_agent_kernel::ExecutorRouter::new();
                executors.add_executor(Arc::new(executor));
                Ok(Kernel::new(
                    Arc::clone(&kernel_store) as Arc<dyn aura_store_db::Store>,
                    provider,
                    executors,
                    KernelConfig {
                        workspace_base,
                        use_workspace_base_as_root: true,
                        tool_approval_prompter: Some(Arc::clone(&prompter)
                            as Arc<dyn aura_agent_kernel::ToolApprovalPrompter>),
                        record_observer: Some(Arc::clone(&record_observer)),
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
        dispatcher: captured.unwrap(),
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

fn png(data: &str) -> aura_ui_terminal::events::ImageAttachment {
    aura_ui_terminal::events::ImageAttachment {
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
        .filter(|m| m.role == aura_model::Role::User)
        .map(aura_model::Message::text_content)
        .collect()
}

#[tokio::test]
async fn message_to_a_busy_agent_waits_in_the_queue() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow start", true), &links).await;
    handle_ui_event(&mut state, message(first, "follow-up", false), &links).await;
    assert_eq!(state.queued.len(first), 1, "the message waits its turn");

    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    assert!(finished.unseen.is_empty());
    handle_work_done(&mut state, finished).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    assert_eq!(user_texts(&state, first), ["slow start", "follow-up"]);
    let shown = drain(&mut commands)
        .into_iter()
        .any(|(_, c)| matches!(c, UiCommand::TurnStarted { prompt: Some(p) } if p == "follow-up"));
    assert!(shown, "the chat shows the message when its turn starts");
}

#[tokio::test]
async fn send_queued_now_cancels_the_turn_and_runs_the_message_next() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow start", true), &links).await;
    handle_ui_event(&mut state, message(first, "urgent", false), &links).await;
    handle_ui_event(&mut state, message(first, "later", false), &links).await;
    assert_eq!(state.queued.len(first), 2);

    // The slow turn is never released: only the cancellation ends it. The
    // cancelled turn leaves "slow start" as the last user message and the
    // next prompt joins it, so the mock holds the later turns too; these
    // permits let them through.
    handle_ui_event(&mut state, UiEvent::SendQueuedNow(first.to_hex()), &links).await;
    let finished = next_done(&mut done).await;
    fixture.gate.add_permits(2);
    handle_work_done(&mut state, finished).await;

    assert!(state.hub.is_running(first), "the queued message started");
    assert_eq!(state.queued.len(first), 1, "only the first one ran");
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    let texts = user_texts(&state, first);
    let position = |text: &str| texts.iter().position(|t| t == text);
    assert!(position("urgent") < position("later"), "{texts:?}");
    assert!(state.queued.is_empty());
}

#[tokio::test]
async fn send_queued_now_with_nothing_queued_leaves_the_turn_alone() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow start", true), &links).await;
    handle_ui_event(&mut state, UiEvent::SendQueuedNow(first.to_hex()), &links).await;

    assert!(
        state.hub.is_running(first),
        "nothing to send, nothing cancelled"
    );
    fixture.gate.add_permits(1);
}

/// Quitting cancels a running turn and waits for it to end, so it saves
/// the session as every turn does. A turn dropped at quit loses all it
/// did since the last checkpoint, and an agent still on its first turn
/// comes back with no session at all.
#[tokio::test]
async fn shutting_down_mid_turn_saves_the_running_turns_session() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let checkpoints = fixture.hub.factory.checkpoints.clone().unwrap();
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow job", true), &links).await;
    assert!(state.hub.is_running(first));
    assert!(
        checkpoints.load_current(first).is_none(),
        "nothing is saved before the first turn ends"
    );

    // The slow turn is never released: only the shutdown's cancel ends it.
    shut_down(&mut state, &mut done).await;

    assert_eq!(state.in_flight, 0, "the turn ended within the grace");
    let saved = checkpoints
        .load_current(first)
        .expect("the cancelled turn saved its session");
    let prompts: Vec<String> = saved
        .messages
        .iter()
        .filter(|m| m.role == aura_model::Role::User)
        .map(aura_model::Message::text_content)
        .collect();
    assert_eq!(prompts, ["slow job"]);
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
        .find(|m| m.role == aura_model::Role::User)
        .unwrap();
    assert!(matches!(
        prompt.content.as_slice(),
        [
            aura_model::ContentBlock::Image { .. },
            aura_model::ContentBlock::Text { text },
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
    assert_eq!(state.queued.len(first), 1, "a turn can't take images");

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
    assert_eq!(
        lock_registry_agent(&fixture, first).title,
        None,
        "only typed prompts title a chat"
    );

    let prompts = user_texts(&state, first);
    assert!(!prompts[0].contains("remember the milk"), "{prompts:?}");
}

#[tokio::test]
async fn the_first_turn_titles_a_placeholder_named_agent() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let (first, second) = (fixture.first, fixture.second);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "fix the build", true), &links).await;
    // The title lands while the turn runs, not once it has ended.
    let title = wait_for_title(&fixture, first).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(
        lock_registry_agent(&fixture, first).title.as_deref(),
        Some(title.as_str())
    );
    assert_eq!(state.idle[&first].messages.len(), 2);
    let shown = state.hub.agent_summaries();
    let row = shown.iter().find(|a| a.id == first.to_hex()).unwrap();
    assert_eq!(row.name, title);

    handle_ui_event(&mut state, message(first, "and again", true), &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(
        lock_registry_agent(&fixture, first).title.as_deref(),
        Some(title.as_str()),
        "an agent is titled once"
    );

    handle_ui_event(&mut state, message(second, "quick question", true), &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(lock_registry_agent(&fixture, second).title, None);
}

/// Wait for the background title job to name `agent_id`.
async fn wait_for_title(fixture: &Fixture, agent_id: AgentId) -> String {
    for _ in 0..500 {
        if let Some(title) = lock_registry_agent(fixture, agent_id).title {
            return title;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the agent was never titled");
}

/// A table whose process manager runs `true` as a background command of
/// `agent_id`, and the completion it reported.
async fn finished_command(
    agent_id: AgentId,
    stopped_by_owner: bool,
) -> (Arc<ProcessTable>, aura_core_types::ProcessId, Transaction) {
    let (tx, mut completions) = mpsc::channel(4);
    let manager = Arc::new(aura_agent::ProcessManager::new(
        tx,
        aura_agent::ProcessManagerConfig::default(),
    ));
    let table = ProcessTable::without_os_scan(None, Some(Arc::clone(&manager)));
    let process_id = aura_core_types::ProcessId::generate();
    manager.register_with(
        agent_id,
        aura_core_types::Hash::from_content(b"proposal"),
        aura_core_types::ActionId::generate(),
        process_id,
        std::process::Command::new("true").spawn().unwrap(),
        "true".to_string(),
        aura_agent::ProcessMeta::default(),
    );
    let info = manager
        .snapshot()
        .into_iter()
        .find(|info| info.process_id == process_id)
        .unwrap();
    table.command_started(&info);
    if stopped_by_owner {
        table.stopped_by_owner(process_id);
    }
    let completion = tokio::time::timeout(Duration::from_secs(10), completions.recv())
        .await
        .unwrap()
        .unwrap();
    (table, process_id, completion)
}

#[tokio::test]
async fn finished_command_starts_a_turn_for_an_idle_agent() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let (table, process_id, completion) = finished_command(first, false).await;
    state.processes = table;

    handle_completion(&mut state, completion).await;
    assert!(state.hub.is_running(first));

    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let prompts = user_texts(&state, first);
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].starts_with(SteerSource::Harness.prefix()));
    assert!(
        prompts[0].contains("`true`") && prompts[0].contains(&process_id.to_hex()),
        "{prompts:?}"
    );
}

#[tokio::test]
async fn a_command_its_agent_stopped_needs_no_reply() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let (table, _, completion) = finished_command(first, true).await;
    state.processes = table;

    handle_completion(&mut state, completion).await;

    assert!(!state.hub.is_running(first));
    assert!(state.queued.is_empty());
}

#[tokio::test]
async fn gate_re_routes_a_turn_whose_route_refuses_the_sign_in() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = ModelSelection::signed_in(
        &fixture.config,
        aura_model::model_catalog::GATE_MODEL_ID,
        aura_auth::AuthSummary {
            subscriptions: vec![aura_auth::ProviderKind::Claude],
            ..aura_auth::AuthSummary::default()
        },
    );
    assert_eq!(state.models.turn_model(), REFUSING_MODEL);

    handle_ui_event(&mut state, message(first, "hello", true), &links).await;
    let finished = next_done(&mut done).await;
    assert_eq!(
        finished.failure, None,
        "a refused route is not a failed turn"
    );
    assert_eq!(finished.refused_models, [REFUSING_MODEL]);
    handle_work_done(&mut state, finished).await;

    assert_eq!(
        state.models.turn_model(),
        "claude/claude-opus-4-7",
        "the refused model stays out of the next turns"
    );
    let replies: Vec<String> = state.idle[&first]
        .messages
        .iter()
        .map(|m| m.text_content())
        .collect();
    assert_eq!(replies, ["hello", "Done."]);
    let shown: Vec<String> = drain(&mut commands)
        .into_iter()
        .filter_map(|(_, c)| match c {
            UiCommand::ShowMessage(m) if m.content.contains("gate → ") => Some(m.content),
            _ => None,
        })
        .collect();
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert!(shown[0].contains("gate → claude-sonnet-4-6 (claude plan)"));
    assert!(shown[1].contains("gate → claude-opus-4-7 (claude plan)"));
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
            refused_models: Vec::new(),
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

/// Archiving is reversible, so it does not refuse an agent with work in
/// flight: it stops that work and archives anyway.
#[tokio::test]
async fn archiving_a_busy_agent_stops_its_turn() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow job", true), &links).await;
    assert!(state.hub.is_running(first));

    handle_ui_event(&mut state, UiEvent::ArchiveAgent(first.to_hex()), &links).await;
    assert!(
        subagents::lock(&state.hub.registry)
            .find(first)
            .unwrap()
            .archived,
        "archived right away, not blocked on the busy turn"
    );
    let stopped = drain(&mut commands).into_iter().any(|(_, c)| {
        matches!(c, UiCommand::ShowSuccess(text) if text.contains("Archived") && text.contains("stopped"))
    });
    assert!(stopped, "the success message says the turn was stopped");

    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, first);
    handle_work_done(&mut state, finished).await;
    assert!(!state.hub.is_running(first), "the stopped turn ended");
}

/// Index of the first command `pick` matches.
fn position_of(
    commands: &[(Option<String>, UiCommand)],
    pick: impl Fn(&UiCommand) -> bool,
) -> Option<usize> {
    commands.iter().position(|(_, c)| pick(c))
}

/// A new agent is in the list the UI gets before its chat and records,
/// so it shows up without waiting on its history.
#[tokio::test]
async fn a_created_agent_is_listed_before_its_history_is_sent() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let _ = drain(&mut commands);

    handle_ui_event(
        &mut state,
        UiEvent::CreateAgent(Some("Fresh".into())),
        &links,
    )
    .await;
    let created = state.active.to_hex();
    let sent = drain(&mut commands);
    let listed = position_of(
        &sent,
        |c| matches!(c, UiCommand::SetAgents(agents) if agents.iter().any(|a| a.id == created)),
    )
    .expect("the new agent is listed");
    let history = position_of(&sent, |c| matches!(c, UiCommand::LoadConversation(_)))
        .expect("its chat is sent");
    let records = position_of(&sent, |c| matches!(c, UiCommand::SetRecords(_)))
        .expect("its records are sent");
    assert!(listed < history && listed < records, "{sent:?}");
    assert!(
        !sent
            .iter()
            .any(|(_, c)| matches!(c, UiCommand::GraphUpsert { .. })),
        "an agent with no record has no stored run to show"
    );
}

/// Archiving the agent on screen takes it off the list before the next
/// agent's chat is read.
#[tokio::test]
async fn archiving_the_active_agent_lists_it_gone_before_opening_the_next() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    assert_eq!(state.active, first);
    let _ = drain(&mut commands);

    handle_ui_event(&mut state, UiEvent::ArchiveAgent(first.to_hex()), &links).await;
    assert_ne!(state.active, first);
    let gone = first.to_hex();
    let sent = drain(&mut commands);
    let listed = position_of(
        &sent,
        |c| matches!(c, UiCommand::SetAgents(agents) if agents.iter().all(|a| a.id != gone)),
    )
    .expect("the list without it is sent");
    let history = position_of(&sent, |c| matches!(c, UiCommand::LoadConversation(_)))
        .expect("the next agent's chat is sent");
    assert!(listed < history, "{sent:?}");
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
    assert!(
        commands.iter().any(|(agent, c)| {
            agent.as_deref() == Some(first.to_hex().as_str()) && matches!(c, UiCommand::ChatReady)
        }),
        "the UI is told its live chat is current, so it stops waiting: {commands:?}"
    );

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
    let history: Vec<aura_model::Message> = std::iter::once(aura_model::Message::user("build it"))
        .chain((0..12).flat_map(|i| {
            [
                aura_model::Message::assistant(format!("step {i}: {}", "x".repeat(400))),
                aura_model::Message::user(format!("ok {i}")),
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
            UiCommand::SetQueue(items) => Some(items.len()),
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
            UiCommand::SetQueue(items) => Some(items.len()),
            _ => None,
        })
        .collect();
    assert_eq!(queued, [0]);
}

#[cfg(unix)]
#[tokio::test]
async fn stop_command_ends_a_running_command_and_reports_it() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let stop = || UiEvent::StopCommand(first.to_hex());
    handle_ui_event(&mut state, stop(), &links).await;
    assert!(drain(&mut commands)
        .iter()
        .any(|(_, c)| matches!(c, UiCommand::SetStatus(s) if s == "No command running")));

    let command = UiEvent::RunCommand {
        agent_id: first.to_hex(),
        command: "echo started; sleep 30".to_string(),
        cols: 80,
    };
    handle_ui_event(&mut state, command, &links).await;
    let started = std::time::Instant::now();
    loop {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "command never started"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        handle_ui_event(&mut state, stop(), &links).await;
        if drain(&mut commands)
            .iter()
            .any(|(_, c)| matches!(c, UiCommand::SetStatus(s) if s == "Stopping the command"))
        {
            break;
        }
    }

    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert!(started.elapsed() < Duration::from_secs(10));
    let output = drain(&mut commands)
        .into_iter()
        .find_map(|(_, c)| match c {
            UiCommand::CommandFinished { output, .. } => Some(output),
            _ => None,
        })
        .expect("command finished");
    assert!(output.contains("[STOPPED]"), "{output:?}");
}

/// A tool call with its result, as a turn leaves them in the history.
fn tool_exchange(id: &str) -> [aura_model::Message; 2] {
    use aura_model::{ContentBlock, Message, Role, ToolResultContent};
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

fn todo(content: &str) -> aura_exec_tools::todo::TodoItem {
    aura_exec_tools::todo::TodoItem {
        id: "1".to_string(),
        content: content.to_string(),
        status: aura_exec_tools::todo::TodoStatus::InProgress,
        assignee: None,
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
        .any(|b| matches!(b, aura_model::ContentBlock::ToolUse { .. })));
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
    session.messages = vec![aura_model::Message::user("old task")];
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
            aura_model::ContentBlock::ToolResult { is_error, .. } => Some(*is_error),
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
        .find_map(|e| e.tx.mode_change_payload())
        .expect("a mode_change record for the switch");
    assert_eq!(switch.from_permissions.as_deref(), Some("agent/ask"));
    assert_eq!(switch.to_permissions.as_deref(), Some("plan/ask"));
    assert_eq!(switch.to, aura_core_types::AgentMode::Plan);

    handle_ui_event(&mut state, UiEvent::CycleApproval(first.to_hex()), &links).await;
    assert_eq!(
        state.hub.permissions_of(first).approval,
        ApprovalLevel::Strict
    );
    assert_eq!(state.hub.mode_of(first), PermissionMode::Plan);
    let records = kernel.store().scan_record(first, 0, 100).unwrap();
    let last = records
        .iter()
        .filter_map(|e| e.tx.mode_change_payload())
        .last()
        .unwrap();
    assert_eq!(last.to_permissions.as_deref(), Some("plan/strict"));
    assert_eq!(kernel.policy().approval(), ApprovalLevel::Strict);
    let shown: Vec<UiCommand> = drain(&mut commands)
        .into_iter()
        .filter(|(to, _)| *to == Some(first.to_hex()))
        .map(|(_, c)| c)
        .filter(|c| matches!(c, UiCommand::SetMode(_) | UiCommand::SetApproval(_)))
        .collect();
    assert!(
        matches!(
            shown.as_slice(),
            [
                UiCommand::SetMode(PermissionMode::Plan),
                UiCommand::SetApproval(ApprovalLevel::Strict)
            ]
        ),
        "{shown:?}"
    );
}

/// Whether the Agents panel rows say each agent is in plan mode, by hex
/// id: from the lists sent to the UI, then from the hub's summaries.
fn plan_marks(hub: &subagents::AgentHub) -> [HashMap<String, bool>; 2] {
    let marks = |rows: Vec<aura_ui_terminal::events::AgentSummary>| {
        rows.into_iter()
            .map(|row| (row.id, row.plan_mode))
            .collect::<HashMap<_, _>>()
    };
    let listed = hub
        .agent_lists()
        .into_iter()
        .find_map(|command| match command {
            UiCommand::SetAgents(rows) => Some(rows),
            _ => None,
        })
        .expect("the lists hold the Agents panel rows");
    [marks(listed), marks(hub.agent_summaries())]
}

#[tokio::test]
async fn a_switch_to_plan_mode_marks_the_agent_in_the_agent_lists() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let (first, second) = (fixture.first.to_hex(), fixture.second.to_hex());
    let mut state = loop_state(&fixture, &mut b);
    let expect = |first_on: bool, second_on: bool| {
        let marks = HashMap::from([(first.clone(), first_on), (second.clone(), second_on)]);
        [marks.clone(), marks]
    };
    assert_eq!(plan_marks(&state.hub), expect(false, false));

    let set_plan = UiEvent::SetMode {
        agent_id: first.clone(),
        mode: PermissionMode::Plan,
    };
    handle_ui_event(&mut state, set_plan, &links).await;
    assert_eq!(plan_marks(&state.hub), expect(true, false));

    // `/plan` again leaves plan mode, and once more enters it.
    handle_ui_event(&mut state, UiEvent::TogglePlan(first.clone()), &links).await;
    assert_eq!(plan_marks(&state.hub), expect(false, false));
    handle_ui_event(&mut state, UiEvent::TogglePlan(first.clone()), &links).await;
    assert_eq!(plan_marks(&state.hub), expect(true, false));
}

#[tokio::test]
async fn a_mode_switch_and_a_request_send_the_agents_policies() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let policies_of = |commands: &mut mpsc::Receiver<UiCommand>| -> Vec<PolicyView> {
        drain(commands)
            .into_iter()
            .filter(|(to, _)| *to == Some(first.to_hex()))
            .filter_map(|(_, c)| match c {
                UiCommand::SetPolicies(view) => Some(view),
                _ => None,
            })
            .collect()
    };
    drain(&mut commands);

    let set_plan = UiEvent::SetMode {
        agent_id: first.to_hex(),
        mode: PermissionMode::Plan,
    };
    handle_ui_event(&mut state, set_plan, &links).await;
    let sent = policies_of(&mut commands);
    let [view] = sent.as_slice() else {
        panic!("expected one SetPolicies after the switch, got {sent:?}");
    };
    assert_eq!(view.mode, PermissionMode::Plan);
    assert_eq!(view.approval, ApprovalLevel::Ask);
    assert_eq!(
        view.policy_hash,
        state.idle[&first].kernel.policy().snapshot_hash()
    );
    let write = view
        .tools
        .iter()
        .find(|tool| tool.name == "write_file")
        .expect("write_file is offered");
    assert_eq!(write.state, ToolPolicyState::Off);

    handle_ui_event(&mut state, UiEvent::RequestPolicies(first.to_hex()), &links).await;
    let sent = policies_of(&mut commands);
    assert_eq!(
        sent.as_slice(),
        std::slice::from_ref(view),
        "a request resends the same view"
    );
}

#[tokio::test]
async fn plan_again_returns_to_the_mode_from_before() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let agent = first.to_hex();

    handle_ui_event(
        &mut state,
        UiEvent::SetMode {
            agent_id: agent.clone(),
            mode: PermissionMode::Debug,
        },
        &links,
    )
    .await;
    handle_ui_event(&mut state, UiEvent::TogglePlan(agent.clone()), &links).await;
    assert_eq!(state.hub.mode_of(first), PermissionMode::Plan);
    assert_eq!(state.idle[&first].kernel.policy().mode_label(), "plan");

    handle_ui_event(&mut state, UiEvent::TogglePlan(agent), &links).await;
    assert_eq!(state.hub.mode_of(first), PermissionMode::Debug);
    assert_eq!(state.idle[&first].kernel.policy().mode_label(), "debug");
}

#[tokio::test]
async fn yolo_runs_commands_without_asking() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let yolo = UiEvent::SetApproval {
        agent_id: first.to_hex(),
        level: ApprovalLevel::Yolo,
    };
    if !crate::event_loop::modes::commands_confinable() {
        return;
    }

    handle_ui_event(&mut state, yolo, &links).await;
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
    use aura_model::Message;

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
    state.idle.get_mut(&first).unwrap().messages =
        vec![aura_model::Message::user("summary of earlier work")];
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

/// Every entry in `agent_id`'s record, oldest first.
fn record_of(state: &LoopState<'_>, agent_id: AgentId) -> Vec<aura_core_types::RecordEntry> {
    state
        .kernel_for(agent_id)
        .unwrap()
        .store()
        .scan_record(agent_id, 1, 10_000)
        .unwrap()
}

/// The prompts in `agent_id`'s record, oldest first.
fn prompts_of(state: &LoopState<'_>, agent_id: AgentId) -> Vec<(u64, Hash, PromptPayload)> {
    record_of(state, agent_id)
        .into_iter()
        .filter_map(|e| e.tx.prompt_payload().map(|p| (e.seq, e.tx.hash, p)))
        .collect()
}

/// The outcomes in `agent_id`'s record, oldest first.
fn outcomes_of(state: &LoopState<'_>, agent_id: AgentId) -> Vec<(u64, OutcomePayload)> {
    record_of(state, agent_id)
        .into_iter()
        .filter_map(|e| e.tx.outcome_payload().map(|o| (e.seq, o)))
        .collect()
}

/// Run a turn answering `text` on `agent_id` to its end.
async fn turn(
    state: &mut LoopState<'_>,
    done: &mut mpsc::Receiver<WorkDone>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    agent_id: AgentId,
    text: &str,
) -> WorkDone {
    handle_ui_event(state, message(agent_id, text, true), links).await;
    let finished = next_done(done).await;
    let reported = WorkDone {
        agent_id: finished.agent_id,
        session: None,
        failure: finished.failure.clone(),
        unseen: Vec::new(),
        refused_models: finished.refused_models.clone(),
    };
    handle_work_done(state, finished).await;
    reported
}

#[tokio::test]
async fn a_pinned_turn_records_its_route_and_nothing_of_gate() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    turn(&mut state, &mut done, &links, first, "fix the parser").await;

    let prompts = prompts_of(&state, first);
    assert_eq!(prompts.len(), 1);
    let (_, _, prompt) = &prompts[0];
    assert_eq!(prompt.text, "fix the parser");
    assert_eq!(prompt.category, None, "a pinned turn is not classified");
    assert!(prompt.classifier.is_empty());
    assert_eq!(prompt.difficulty, None);
    assert_eq!(prompt.route, Route::turn("mock-model", RouteReason::Pinned));
    assert!(outcomes_of(&state, first).is_empty());
    assert!(state.hub.gate.recent().is_empty());
}

#[tokio::test]
async fn gate_classifies_before_the_turn_and_records_its_default_route() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = ModelSelection::signed_in(
        &fixture.config,
        aura_model::model_catalog::GATE_MODEL_ID,
        aura_auth::AuthSummary {
            zos_user: Some("alice".to_string()),
            ..aura_auth::AuthSummary::default()
        },
    );

    turn(&mut state, &mut done, &links, first, "what does gate do?").await;

    let (prompt_seq, _, prompt) = prompts_of(&state, first).remove(0);
    assert_eq!(prompt.category, Some(PromptCategory::Question));
    assert_eq!(prompt.difficulty, Some(aura_core_types::Difficulty::Medium));
    assert_eq!(prompt.classifier, "aura-claude-haiku-4-5");
    assert_eq!(
        prompt.route,
        Route::turn("aura-claude-sonnet-4-6", RouteReason::Default).at_effort("medium")
    );
    // The title is written in the background while the turn runs, so its
    // call races the turn's own and is told apart by purpose, not by
    // position in the record.
    let calls: Vec<(u64, Option<aura_model::ModelCallPurpose>)> = record_of(&state, first)
        .iter()
        .filter(|e| e.tx.tx_type == aura_core_types::TransactionType::Reasoning)
        .map(|e| {
            let purpose = json_payload(&e.tx)
                .and_then(|payload| payload.get("purpose").cloned())
                .and_then(|purpose| serde_json::from_value(purpose).ok());
            (e.seq, purpose)
        })
        .collect();
    assert_eq!(
        calls.len(),
        4,
        "the classifier, the turn, the judge of a turn that checked nothing \
         and the chat title: {calls:?}"
    );
    let classifier_seq = calls
        .iter()
        .find(|(_, purpose)| *purpose == Some(aura_model::ModelCallPurpose::Classify))
        .map(|(seq, _)| *seq)
        .expect("the classifier call is recorded");
    let turn_seq = calls
        .iter()
        .find(|(_, purpose)| purpose.is_none())
        .map(|(seq, _)| *seq)
        .expect("the turn's own call is recorded");
    assert!(classifier_seq < prompt_seq && prompt_seq < turn_seq);
    let (_, outcome) = outcomes_of(&state, first).remove(0);
    assert_eq!(outcome.model, "aura-claude-sonnet-4-6");
}

#[tokio::test]
async fn a_failing_classifier_fails_the_routed_turn() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = ModelSelection::signed_in(
        &fixture.config,
        aura_model::model_catalog::GATE_MODEL_ID,
        aura_auth::AuthSummary {
            zos_user: Some("alice".to_string()),
            ..aura_auth::AuthSummary::default()
        },
    );

    turn(&mut state, &mut done, &links, first, "classifier down?").await;

    let shown = drain(&mut commands);
    assert!(
        shown.iter().any(|(_, c)| matches!(
            c,
            UiCommand::ShowError(problem) if problem.contains("could not classify")
        )),
        "the turn fails with why"
    );
    assert!(prompts_of(&state, first).is_empty());
    assert!(outcomes_of(&state, first).is_empty());
}

#[tokio::test]
async fn a_classifier_reply_without_a_label_is_asked_again_then_fails() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = ModelSelection::signed_in(
        &fixture.config,
        aura_model::model_catalog::GATE_MODEL_ID,
        aura_auth::AuthSummary {
            zos_user: Some("alice".to_string()),
            ..aura_auth::AuthSummary::default()
        },
    );

    turn(&mut state, &mut done, &links, first, "classifier rambles").await;

    let classified = record_of(&state, first)
        .iter()
        .filter(|e| e.tx.tx_type == aura_core_types::TransactionType::Reasoning)
        .filter(|e| {
            json_payload(&e.tx)
                .and_then(|payload| payload.get("purpose").cloned())
                .and_then(|purpose| serde_json::from_value(purpose).ok())
                == Some(aura_model::ModelCallPurpose::Classify)
        })
        .count();
    assert_eq!(classified, 2, "asked once more");
    let shown = drain(&mut commands);
    assert!(
        shown.iter().any(|(_, c)| matches!(
            c,
            UiCommand::ShowError(problem)
                if problem.contains("gave no category and difficulty")
                    && problem.contains("I'll start by reading the code.")
        )),
        "the turn fails with the reply: {shown:?}"
    );
    assert!(prompts_of(&state, first).is_empty());
}

/// `agent` mode at `yolo`: nothing asks.
fn yolo() -> Permissions {
    Permissions::new(PermissionMode::Agent, ApprovalLevel::Yolo)
}

/// Let `agent_id` run commands without asking.
async fn unrestricted(
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    agent_id: AgentId,
) {
    let event = UiEvent::SetApproval {
        agent_id: agent_id.to_hex(),
        level: ApprovalLevel::Yolo,
    };
    handle_ui_event(state, event, links).await;
}

#[tokio::test]
async fn the_last_check_of_a_turn_decides_its_outcome() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    unrestricted(&mut state, &links, first).await;

    turn(&mut state, &mut done, &links, first, "run failing check").await;
    turn(&mut state, &mut done, &links, first, "run passing check").await;

    let asked: Vec<Hash> = prompts_of(&state, first)
        .into_iter()
        .filter(|(_, _, p)| p.route.reason != RouteReason::Escalate)
        .map(|(_, hash, _)| hash)
        .collect();
    let signals: Vec<(OutcomeSignal, Option<bool>)> = outcomes_of(&state, first)
        .into_iter()
        .filter(|(_, o)| asked.contains(&o.prompt_tx))
        .map(|(_, o)| (o.signal, o.success))
        .collect();
    assert_eq!(
        signals,
        [
            (OutcomeSignal::ChecksFailed, Some(false)),
            (OutcomeSignal::ChecksPassed, Some(true)),
        ],
        "the outcomes of the prompts sent, not of the move up the failure made"
    );
}

#[tokio::test]
async fn a_turn_that_fails_records_an_error_outcome() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);

    turn(&mut state, &mut done, &links, first, "explode").await;

    let (_, outcome) = outcomes_of(&state, first).remove(0);
    assert_eq!(outcome.signal, OutcomeSignal::Error);
    assert_eq!(outcome.success, Some(false));
}

#[tokio::test]
async fn rewinding_turns_fails_them() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);

    turn(&mut state, &mut done, &links, first, "first turn").await;
    turn(&mut state, &mut done, &links, first, "second turn").await;
    turn(&mut state, &mut done, &links, first, "third turn").await;
    let prompts: Vec<Hash> = prompts_of(&state, first)
        .into_iter()
        .map(|(_, hash, _)| hash)
        .collect();
    let event = UiEvent::Rewind {
        agent_id: first.to_hex(),
        turn: Some(2),
        scope: RewindScope::Chat,
    };
    handle_ui_event(&mut state, event, &links).await;

    let rewound: Vec<Hash> = outcomes_of(&state, first)
        .into_iter()
        .filter(|(_, o)| o.signal == OutcomeSignal::Rewind)
        .map(|(_, o)| {
            assert_eq!(o.success, Some(false));
            o.prompt_tx
        })
        .collect();
    assert_eq!(rewound, prompts[1..]);
}

#[tokio::test]
async fn the_same_prompt_on_a_stronger_model_fails_the_earlier_turn() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let ask = "add a --json flag to the status command";

    state.models = gate_on_zos(&fixture);
    turn(&mut state, &mut done, &links, first, ask).await;
    turn(&mut state, &mut done, &links, first, ask).await;
    assert!(
        outcomes_of(&state, first)
            .iter()
            .all(|(_, o)| o.signal != OutcomeSignal::RetryStronger),
        "the same model again is not a retry on a stronger one"
    );

    state.models = ModelSelection::fixed(&fixture.config, "aura-claude-opus-4-7");
    turn(&mut state, &mut done, &links, first, &format!("{ask}.")).await;

    let prompts = prompts_of(&state, first);
    let outcomes = outcomes_of(&state, first);
    let retried: Vec<&OutcomePayload> = outcomes
        .iter()
        .map(|(_, o)| o)
        .filter(|o| o.signal == OutcomeSignal::RetryStronger)
        .collect();
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0].prompt_tx, prompts[1].1, "the turn just before");
    assert_eq!(retried[0].model, "aura-claude-sonnet-4-6");
    assert_eq!(retried[0].success, Some(false));
    assert_eq!(prompts[2].2.route.model, "aura-claude-opus-4-7");
}

/// Gate selected with a zOS sign-in: the router's models, no local ones.
fn gate_on_zos(fixture: &Fixture) -> ModelSelection {
    ModelSelection::signed_in(
        &fixture.config,
        aura_model::model_catalog::GATE_MODEL_ID,
        aura_auth::AuthSummary {
            zos_user: Some("alice".to_string()),
            ..aura_auth::AuthSummary::default()
        },
    )
}

/// Feed `live` `n` question turns on `model` that passed
/// their checks, recorded by some other agent.
fn learn_questions(live: &GateLive, model: &str, n: usize) {
    use aura_agent_kernel::RecordObserver;
    let agent = AgentId::generate();
    for i in 0..n {
        let mut prompt = PromptPayload::new(format!("earlier question {i}"));
        prompt.category = Some(PromptCategory::Question);
        prompt.route = Route::turn(model, RouteReason::Default);
        let prompt_tx = Transaction::user_prompt_with(agent, &prompt);
        let outcome = OutcomePayload {
            prompt_tx: prompt_tx.hash,
            category: PromptCategory::Question,
            model: model.to_string(),
            success: Some(true),
            signal: OutcomeSignal::ChecksPassed,
            cost_usd: 0.001,
            subtask_cost_usd: 0.0,
            latency_ms: 10,
        };
        let seq = 2 * u64::try_from(i).unwrap();
        live.on_entry(&aura_core_types::RecordEntry::builder(seq + 1, prompt_tx).build());
        live.on_entry(
            &aura_core_types::RecordEntry::builder(seq + 2, Transaction::outcome(agent, &outcome))
                .build(),
        );
    }
}

/// The chat lines saying where Gate sent turns.
fn route_lines(commands: &mut mpsc::Receiver<UiCommand>) -> Vec<String> {
    drain(commands)
        .into_iter()
        .filter_map(|(_, c)| match c {
            UiCommand::ShowMessage(m) if m.content.contains("gate → ") => Some(m.content),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn gate_routes_each_category_and_records_why() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    learn_questions(&fixture.hub.gate, "aura-claude-haiku-4-5", 10);

    turn(
        &mut state,
        &mut done,
        &links,
        first,
        "what does gate do? (easy)",
    )
    .await;
    turn(&mut state, &mut done, &links, first, "fix the parser").await;

    let prompts = prompts_of(&state, first);
    assert_eq!(
        prompts[0].2.route,
        Route::turn("aura-claude-haiku-4-5", RouteReason::Proven),
        "ten passing questions prove haiku on easy questions, which it answers \
         with thinking off"
    );
    assert_eq!(prompts[1].2.category, Some(PromptCategory::Debugging));
    assert_eq!(
        prompts[1].2.route,
        Route::turn("aura-claude-sonnet-4-6", RouteReason::Default).at_effort("medium"),
        "debugging starts on the standard tier: gpt-5.4 lists cheaper than \
         sonnet, but the frontier is reached only by failing"
    );
    let models: Vec<String> = outcomes_of(&state, first)
        .into_iter()
        .map(|(_, o)| o.model)
        .collect();
    assert_eq!(models, ["aura-claude-haiku-4-5", "aura-claude-sonnet-4-6"]);
    let shown = route_lines(&mut commands);
    assert_eq!(
        shown,
        [
            "gate → aura-claude-haiku-4-5 (cloud) with thinking off · question, proven",
            "gate → aura-claude-sonnet-4-6 (cloud) at medium effort · debugging, default",
        ]
    );
    let recent = fixture.hub.gate.recent();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[1].reason, RouteReason::Default);
}

#[tokio::test]
async fn the_classifiers_difficulty_moves_where_a_turn_starts_and_is_recorded() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);

    turn(
        &mut state,
        &mut done,
        &links,
        first,
        "what does gate do? (easy)",
    )
    .await;
    turn(&mut state, &mut done, &links, first, "what does gate do?").await;
    turn(
        &mut state,
        &mut done,
        &links,
        first,
        "fix the parser (hard)",
    )
    .await;

    use aura_core_types::Difficulty;
    let prompts: Vec<(Option<PromptCategory>, Option<Difficulty>, String)> =
        prompts_of(&state, first)
            .into_iter()
            .map(|(_, _, p)| (p.category, p.difficulty, p.route.model))
            .collect();
    assert_eq!(
        prompts,
        [
            (
                Some(PromptCategory::Question),
                Some(Difficulty::Easy),
                "aura-claude-haiku-4-5".to_string()
            ),
            (
                Some(PromptCategory::Question),
                Some(Difficulty::Medium),
                "aura-claude-sonnet-4-6".to_string()
            ),
            (
                Some(PromptCategory::Debugging),
                Some(Difficulty::Hard),
                "aura-claude-sonnet-4-6".to_string()
            ),
        ],
        "an easy question starts on the fast tier; medium and hard work on \
         the standard one, hard debugging too"
    );
}

/// Run `text`, easy debugging, as a turn under Gate, which starts it on
/// Haiku, and return the routes of its prompts, its outcomes, the chat's
/// system lines, and the tries and successes of the debugging rows of
/// Haiku and of Sonnet, the cheapest model of the next tier up.
async fn debugging_turn_under_gate(
    text: &str,
) -> (
    Vec<Route>,
    Vec<(String, OutcomeSignal)>,
    String,
    [Option<(u32, u32)>; 2],
) {
    debugging_turn_under(steady_gate(), text).await
}

/// [`steady_gate`] that reviews work about to stand as `review` says,
/// every turn a review it only logs, with an attempt log (in the folder
/// returned) in which checked work that changed no file has been wrong
/// often enough to be reviewed.
fn reviewing_gate(review: aura_config::ReviewMode) -> (GateLive, TempDir) {
    use aura_gate::attempts::{Attempt, AttemptLog, Line, Terms, Truth, TruthSource};
    use aura_gate::evidence::Evidence;
    let dir = TempDir::new().unwrap();
    let log = AttemptLog::in_data_dir(dir.path());
    let agent = AgentId::generate().to_hex();
    for n in 0..10u8 {
        let unchanged = n < 5;
        let ended = OutcomePayload {
            prompt_tx: Hash::from_content(&[n]),
            category: PromptCategory::Debugging,
            model: "aura-claude-haiku-4-5".to_string(),
            success: Some(true),
            signal: OutcomeSignal::ChecksPassed,
            cost_usd: 0.01,
            subtask_cost_usd: 0.0,
            latency_ms: 10,
        };
        let terms = Terms {
            difficulty: None,
            led: Some(false),
            routed: true,
        };
        let evidence = Evidence {
            checks_run: 1,
            files_changed: Some(u32::from(!unchanged)),
            reply: "Done.".to_string(),
            ..Evidence::default()
        };
        let attempt = Attempt::new(u64::from(n), agent.clone(), &ended, terms, evidence);
        let truth = Truth {
            at_ms: u64::from(n),
            agent: agent.clone(),
            prompt_tx: attempt.prompt_tx.clone(),
            source: TruthSource::TaskCheck,
            passed: !unchanged,
            compare: None,
            run: None,
            task: None,
            copy: None,
        };
        log.append(&Line::Attempt(attempt)).unwrap();
        log.append(&Line::Truth(truth)).unwrap();
    }
    let gate = GateLive::new(aura_config::GateConfig {
        review,
        review_sample: 1.0,
        ..aura_config::GateConfig::defaults().without_exploring()
    });
    gate.log_attempts_to(log);
    assert_eq!(
        gate.tiers().of(aura_gate::evidence::Flag::NoChanges),
        aura_gate::attempts::Tier::Review
    );
    (gate, dir)
}

/// [`debugging_turn_under_gate`] with Gate's table `gate`.
async fn debugging_turn_under(
    gate: GateLive,
    text: &str,
) -> (
    Vec<Route>,
    Vec<(String, OutcomeSignal)>,
    String,
    [Option<(u32, u32)>; 2],
) {
    let (fixture, mut commands) = fixture_with_gate(gate);
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    unrestricted(&mut state, &links, first).await;
    drain(&mut commands);

    let finished = turn(&mut state, &mut done, &links, first, text).await;
    assert_eq!(finished.failure, None);

    let routes = prompts_of(&state, first)
        .into_iter()
        .map(|(_, _, p)| p.route)
        .collect();
    let outcomes = outcomes_of(&state, first)
        .into_iter()
        .map(|(_, o)| (o.model, o.signal))
        .collect();
    let stats = fixture.hub.gate.stats();
    let row = |model: &str| {
        stats
            .row(RouteLevel::Turn, PromptCategory::Debugging, model)
            .map(|row| (row.tries, row.successes))
    };
    let rows = [row("aura-claude-haiku-4-5"), row("aura-claude-sonnet-4-6")];
    (routes, outcomes, system_texts(&drain(&mut commands)), rows)
}

#[tokio::test]
async fn a_failed_turn_the_judge_blames_on_the_model_carries_on_on_a_stronger_one() {
    const HAIKU: &str = "aura-claude-haiku-4-5";
    const SONNET: &str = "aura-claude-sonnet-4-6";
    let (routes, outcomes, shown, rows) =
        debugging_turn_under_gate("fix it (easy): run failing check (judge: blame)").await;

    assert_eq!(
        routes,
        [
            Route::turn(HAIKU, RouteReason::Default).at_effort("medium"),
            Route::turn(SONNET, RouteReason::Escalate).at_effort("medium"),
        ]
    );
    assert_eq!(
        outcomes,
        [
            (HAIKU.to_string(), OutcomeSignal::ChecksFailed),
            (HAIKU.to_string(), OutcomeSignal::JudgedFail),
            (SONNET.to_string(), OutcomeSignal::Unavailable),
        ]
    );
    assert!(
        shown.contains("the failure is aura-claude-haiku-4-5 (cloud)'s. The change is wrong."),
        "{shown}"
    );
    assert!(
        shown.contains(
            "gate → aura-claude-sonnet-4-6 (cloud) at medium effort · debugging, escalate"
        ),
        "{shown}"
    );
    assert_eq!(
        rows,
        [Some((1, 0)), None],
        "the failure counts once for Haiku; the stronger model's work is not counted"
    );
}

#[tokio::test]
async fn a_passed_check_the_reply_disowns_is_reviewed_and_carries_on_on_a_stronger_model() {
    const HAIKU: &str = "aura-claude-haiku-4-5";
    const SONNET: &str = "aura-claude-sonnet-4-6";
    let (routes, outcomes, shown, rows) = debugging_turn_under_gate(
        "fix it (easy): run passing check, then reply unverified (review: blame)",
    )
    .await;

    assert_eq!(
        routes,
        [
            Route::turn(HAIKU, RouteReason::Default).at_effort("medium"),
            Route::turn(SONNET, RouteReason::Escalate).at_effort("medium"),
        ],
        "{shown}"
    );
    assert_eq!(
        outcomes,
        [
            (HAIKU.to_string(), OutcomeSignal::JudgedFail),
            (SONNET.to_string(), OutcomeSignal::Unavailable),
        ],
        "the check that passed does not count for Haiku"
    );
    assert!(
        shown.contains("The change is untested and wrong."),
        "{shown}"
    );
    assert_eq!(rows, [Some((1, 0)), None]);
}

#[tokio::test]
async fn a_passed_check_the_reply_disowns_stands_when_the_judge_finds_the_work_done() {
    const HAIKU: &str = "aura-claude-haiku-4-5";
    let (routes, outcomes, shown, rows) = debugging_turn_under_gate(
        "fix it (easy): run passing check, then reply unverified (review: fine)",
    )
    .await;

    assert_eq!(
        routes,
        [Route::turn(HAIKU, RouteReason::Default).at_effort("medium")],
        "{shown}"
    );
    assert_eq!(outcomes, [(HAIKU.to_string(), OutcomeSignal::ChecksPassed)]);
    assert!(shown.contains("The change is right."), "{shown}");
    assert_eq!(rows, [Some((1, 1)), None]);
}

#[tokio::test]
async fn hard_work_a_review_finds_unfinished_goes_back_to_its_model_once() {
    use aura_gate::attempts::Line;
    use aura_gate::judge::turn::Answer;
    let (gate, _log_dir) = reviewing_gate(aura_config::ReviewMode::On);
    let (fixture, mut commands) = fixture_with_gate(gate);
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    unrestricted(&mut state, &links, first).await;
    drain(&mut commands);

    let text = "fix it: run passing check (inspect: once)";
    let finished = turn(&mut state, &mut done, &links, first, text).await;
    assert_eq!(finished.failure, None);
    let shown = system_texts(&drain(&mut commands));

    let prompts = prompts_of(&state, first);
    let model = prompts[0].2.route.model.clone();
    assert!(
        prompts
            .iter()
            .all(|(_, _, p)| p.route.reason != RouteReason::Escalate),
        "the work stays on its model: {shown}"
    );
    let sent_back: Vec<&str> = prompts
        .iter()
        .map(|(_, _, p)| p.text.as_str())
        .filter(|text| text.contains(aura_gate::followup::REPAIR_OPENING))
        .collect();
    assert_eq!(sent_back.len(), 1, "{shown}");
    assert!(
        sent_back[0].contains("1. The fix misses the empty case."),
        "{}",
        sent_back[0]
    );
    let outcomes: Vec<(String, OutcomeSignal)> = outcomes_of(&state, first)
        .into_iter()
        .map(|(_, o)| (o.model, o.signal))
        .collect();
    assert_eq!(
        outcomes,
        [(model.clone(), OutcomeSignal::ChecksPassed)],
        "one attempt, which stands once the review finds nothing missing: {shown}"
    );
    for part in [
        "work is not finished.\n1. The fix misses the empty case.",
        "gate: sending the review's findings back to",
        "found nothing missing in",
    ] {
        assert!(shown.contains(part), "{part}: {shown}");
    }

    // Both reviews are in the attempt log, with why they were asked.
    let lines = fixture
        .hub
        .gate
        .attempt_log()
        .unwrap()
        .read()
        .unwrap()
        .lines;
    let reviews: Vec<_> = lines
        .iter()
        .filter_map(|line| match line {
            Line::Review(review) => Some(review),
            _ => None,
        })
        .collect();
    assert_eq!(
        reviews
            .iter()
            .map(|review| review.verdict)
            .collect::<Vec<_>>(),
        [Answer::Model, Answer::Ok]
    );
    for review in &reviews {
        assert_eq!(review.why, ["no_changes"]);
        assert!(review.acted && !review.replayed);
        assert_eq!(review.variant, "plain");
        assert_eq!(review.prompt_tx, prompts[0].1.to_hex());
    }
    assert!(
        lines.iter().any(
            |line| matches!(line, Line::Attempt(attempt) if attempt.prompt_tx == prompts[0].1.to_hex())
        ),
        "{lines:?}"
    );
}

#[tokio::test]
async fn work_a_review_finds_unfinished_twice_moves_to_a_stronger_model() {
    let (gate, _log_dir) = reviewing_gate(aura_config::ReviewMode::On);
    let (routes, outcomes, shown, _) =
        debugging_turn_under(gate, "fix it (easy): run passing check (inspect: blame)").await;

    let first = routes[0].model.clone();
    assert_eq!(
        outcomes[0],
        (first.clone(), OutcomeSignal::JudgedFail),
        "the check that passed does not count for the model: {shown}"
    );
    let moved: Vec<&Route> = routes
        .iter()
        .filter(|route| route.reason == RouteReason::Escalate)
        .collect();
    assert!(!moved.is_empty(), "{shown}");
    assert_ne!(moved[0].model, first);
    assert_eq!(
        shown
            .matches("gate: sending the review's findings back to")
            .count(),
        1,
        "sent back once, on the model it started on: {shown}"
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|(_, signal)| *signal == OutcomeSignal::JudgedFail)
            .count(),
        2,
        "what the next model leaves unfinished moves on again: {outcomes:?}"
    );
}

#[tokio::test]
async fn a_logged_review_changes_nothing_about_the_turn() {
    let text = "fix it: run passing check (inspect: blame)";
    let (gate, _log_dir) = reviewing_gate(aura_config::ReviewMode::Log);
    let (routes, outcomes, shown, _) = debugging_turn_under(gate, text).await;
    assert_eq!(routes.len(), 1, "{shown}");
    assert_eq!(
        outcomes,
        [(routes[0].model.clone(), OutcomeSignal::ChecksPassed)]
    );
    assert!(
        shown.contains("work looks unfinished (logged only; AURA_GATE_REVIEW=on acts on it)."),
        "{shown}"
    );

    let (gate, _log_dir) = reviewing_gate(aura_config::ReviewMode::Off);
    let (routes, outcomes, shown, _) = debugging_turn_under(gate, text).await;
    assert_eq!(routes.len(), 1);
    assert_eq!(
        outcomes,
        [(routes[0].model.clone(), OutcomeSignal::ChecksPassed)]
    );
    assert!(!shown.contains("Gate's review"), "{shown}");
}

#[tokio::test]
async fn a_failed_turn_the_judge_excuses_ends_where_it_is() {
    const HAIKU: &str = "aura-claude-haiku-4-5";
    let (routes, outcomes, shown, rows) =
        debugging_turn_under_gate("fix it (easy): run failing check (judge: excuse)").await;

    assert_eq!(
        routes,
        [Route::turn(HAIKU, RouteReason::Default).at_effort("medium")]
    );
    assert_eq!(
        outcomes,
        [
            (HAIKU.to_string(), OutcomeSignal::ChecksFailed),
            (HAIKU.to_string(), OutcomeSignal::JudgedExcused),
        ]
    );
    assert!(
        shown.contains("the failure is not aura-claude-haiku-4-5 (cloud)'s doing"),
        "{shown}"
    );
    assert!(!shown.contains("escalate"), "{shown}");
    assert_eq!(rows, [None, None], "an excused failure counts for nothing");
}

#[tokio::test]
async fn a_failed_turn_with_no_verdict_carries_on_on_a_stronger_model() {
    const HAIKU: &str = "aura-claude-haiku-4-5";
    const SONNET: &str = "aura-claude-sonnet-4-6";
    let (routes, outcomes, _, rows) =
        debugging_turn_under_gate("fix it (easy): run failing check").await;

    assert_eq!(
        routes,
        [
            Route::turn(HAIKU, RouteReason::Default).at_effort("medium"),
            Route::turn(SONNET, RouteReason::Escalate).at_effort("medium"),
        ]
    );
    assert_eq!(
        outcomes,
        [
            (HAIKU.to_string(), OutcomeSignal::ChecksFailed),
            (SONNET.to_string(), OutcomeSignal::Unavailable),
        ],
        "the turn's own signal stands and is recorded once"
    );
    assert_eq!(rows, [Some((1, 0)), None]);
}

#[tokio::test]
async fn gate_explores_the_next_cheaper_model_and_records_its_propensity() {
    let gate = GateLive::with_rng(
        aura_config::GateConfig {
            explore_rate: 1.0,
            ..aura_config::GateConfig::defaults()
        },
        aura_gate::route::SplitMix64::seeded(3),
    );
    let (fixture, mut commands) = fixture_with_gate(gate);
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);

    turn(&mut state, &mut done, &links, first, "what does gate do?").await;

    let (_, _, prompt) = prompts_of(&state, first).remove(0);
    assert_eq!(
        prompt.route,
        Route {
            explored: true,
            propensity: 1.0,
            ..Route::turn("aura-gpt-5-4", RouteReason::Explore).at_effort("medium")
        },
        "the default for questions is sonnet; the next cheaper is gpt-5.4"
    );
    let (_, outcome) = outcomes_of(&state, first).remove(0);
    assert_eq!(outcome.model, "aura-gpt-5-4");
    assert_eq!(
        route_lines(&mut commands),
        ["gate → aura-gpt-5-4 (cloud) at medium effort · question, explore"]
    );
}

#[tokio::test]
async fn the_next_turn_uses_what_the_last_outcome_taught_gate() {
    let gate = GateLive::new(aura_config::GateConfig {
        min_tries: 1,
        ..aura_config::GateConfig::defaults().without_exploring()
    });
    let (fixture, _commands) = fixture_with_gate(gate);
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    // A later signal about the last turn, as the judge records one.
    async fn rate(state: &LoopState<'_>, agent: AgentId, signal: OutcomeSignal) {
        let kernel = state.kernel_for(agent).unwrap();
        let last = aura_gate::live::last_turn(kernel.store().as_ref(), agent).unwrap();
        assert!(aura_gate::live::record(&kernel, agent, &last.outcome.with_signal(signal)).await);
    }
    let reasons = |state: &LoopState<'_>| -> Vec<RouteReason> {
        prompts_of(state, first)
            .into_iter()
            .map(|(_, _, p)| p.route.reason)
            .collect()
    };

    turn(&mut state, &mut done, &links, first, "what is a monad?").await;
    rate(&state, first, OutcomeSignal::JudgedPass).await;
    turn(&mut state, &mut done, &links, first, "what is a functor?").await;
    assert_eq!(reasons(&state), [RouteReason::Default, RouteReason::Proven]);

    rate(&state, first, OutcomeSignal::JudgedFail).await;
    let row = fixture
        .hub
        .gate
        .stats()
        .row(
            RouteLevel::Turn,
            PromptCategory::Question,
            "aura-claude-sonnet-4-6",
        )
        .copied()
        .unwrap();
    assert_eq!(
        (row.tries, row.successes),
        (2, 1),
        "each prompt counts once"
    );
    turn(
        &mut state,
        &mut done,
        &links,
        first,
        "what is an applicative?",
    )
    .await;
    assert_eq!(
        reasons(&state),
        [
            RouteReason::Default,
            RouteReason::Proven,
            RouteReason::Default
        ],
        "one of two is below the target, so sonnet drops back to the default"
    );
}

#[tokio::test]
async fn a_criteria_writer_that_fails_says_so_and_the_turn_goes_on() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);

    turn(
        &mut state,
        &mut done,
        &links,
        first,
        "fix the parser (criteria writer down)",
    )
    .await;

    let text = system_texts(&drain(&mut commands));
    assert!(
        text.contains(
            "gate: aura-claude-sonnet-4-6 (cloud) wrote no acceptance criteria: the call failed ("
        ),
        "{text}"
    );
    assert!(text.contains("not supported for this model"), "{text}");
    assert!(
        text.contains("gate → "),
        "the turn is routed all the same:\n{text}"
    );
    assert_eq!(outcomes_of(&state, first).len(), 1, "and runs to its end");
}

/// The difficulty the prompt of a turn answering `text` is recorded with,
/// under Gate with acceptance criteria `criteria`.
async fn recorded_difficulty(
    criteria: aura_config::ReviewMode,
    text: &str,
) -> Option<aura_core_types::Difficulty> {
    let (fixture, _commands) = fixture_with_gate(GateLive::new(aura_config::GateConfig {
        explore_rate: 0.0,
        criteria,
        ..aura_config::GateConfig::defaults()
    }));
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    turn(&mut state, &mut done, &links, first, text).await;
    prompts_of(&state, first)[0].2.difficulty
}

#[tokio::test]
async fn exacting_criteria_rate_the_work_hard_unless_criteria_are_off() {
    use aura_config::ReviewMode;
    use aura_core_types::Difficulty;
    let exacting = "fix the parser (exacting criteria)";
    assert_eq!(
        recorded_difficulty(ReviewMode::Log, exacting).await,
        Some(Difficulty::Hard),
        "logged criteria route the work as acting ones do"
    );
    assert_eq!(
        recorded_difficulty(ReviewMode::On, exacting).await,
        Some(Difficulty::Hard)
    );
    assert_eq!(
        recorded_difficulty(ReviewMode::Off, exacting).await,
        Some(Difficulty::Medium),
        "no criteria are written"
    );
    assert_eq!(
        recorded_difficulty(ReviewMode::Log, "fix the parser (plain criteria)").await,
        Some(Difficulty::Medium),
        "criteria that are not exacting leave the classifier's difficulty"
    );
}

/// What the model doing a turn answering `text` was sent as its request,
/// under Gate with acceptance criteria `criteria`.
async fn request_sent(criteria: aura_config::ReviewMode, text: &str) -> String {
    let (fixture, _commands) = fixture_with_gate(GateLive::new(aura_config::GateConfig {
        explore_rate: 0.0,
        criteria,
        ..aura_config::GateConfig::defaults()
    }));
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    turn(&mut state, &mut done, &links, first, text).await;
    let sent = user_texts(&state, first).remove(0);
    assert_eq!(
        prompts_of(&state, first)[0].2.text,
        text,
        "the record keeps the prompt as typed"
    );
    sent
}

#[tokio::test]
async fn the_worker_is_shown_the_criteria_unless_criteria_are_off() {
    use aura_config::ReviewMode;
    let text = "fix the parser (plain criteria)";
    let note = "the harness wrote down what this work will be checked against";
    let criterion = "prints the parse tree";
    for mode in [ReviewMode::Log, ReviewMode::On] {
        let sent = request_sent(mode, text).await;
        assert!(sent.starts_with(text), "{mode}: {sent}");
        assert!(
            sent.contains(note) && sent.contains(criterion),
            "{mode}: {sent}"
        );
    }
    assert_eq!(request_sent(ReviewMode::Off, text).await, text);
    let unlisted = "fix the parser";
    assert_eq!(
        request_sent(ReviewMode::Log, unlisted).await,
        unlisted,
        "nothing is added when the writer listed none"
    );
}

#[tokio::test]
async fn slash_gate_shows_the_table() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    learn_questions(&fixture.hub.gate, "aura-claude-haiku-4-5", 10);

    handle_ui_event(&mut state, UiEvent::ShowGate, &links).await;

    let text = system_texts(&drain(&mut commands));
    assert!(text.contains("Turns"), "{text}");
    assert!(
        text.contains("question            → aura-claude-sonnet-4-6 (cloud)"),
        "{text}"
    );
    assert!(text.contains("→ aura-claude-haiku-4-5 (cloud)"), "{text}");
    assert!(text.contains("pick for easy work (proven)"), "{text}");
}

#[tokio::test]
async fn focusing_an_agent_makes_it_active_without_reloading_its_chat() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let (first, second) = (fixture.first, fixture.second);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, UiEvent::SelectAgent(second.to_hex()), &links).await;
    let selected = drain(&mut commands);
    assert!(
        selected
            .iter()
            .any(|(_, c)| matches!(c, UiCommand::LoadConversation(_))),
        "selecting an idle agent loads its chat"
    );

    handle_ui_event(&mut state, UiEvent::FocusAgent(first.to_hex()), &links).await;
    assert_eq!(state.active, first);
    let focused = drain(&mut commands);
    assert!(matches!(
        focused.first(),
        Some((None, UiCommand::SetActiveAgent(id))) if *id == first.to_hex()
    ));
    assert!(
        focused
            .iter()
            .any(|(_, c)| matches!(c, UiCommand::SetRecords(_))),
        "the Record panel follows the focused agent"
    );
    assert!(
        !focused
            .iter()
            .any(|(_, c)| matches!(c, UiCommand::LoadConversation(_))),
        "the view that asked already has the chat"
    );
}

#[tokio::test]
async fn a_selected_agent_gets_its_chat_before_its_records() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(
        &mut state,
        UiEvent::SelectAgent(fixture.second.to_hex()),
        &links,
    )
    .await;
    let sent = drain(&mut commands);
    let at = |wanted: fn(&UiCommand) -> bool| sent.iter().position(|(_, c)| wanted(c));
    let chat = at(|c| matches!(c, UiCommand::LoadConversation(_))).expect("chat sent");
    let records = at(|c| matches!(c, UiCommand::SetRecords(_))).expect("records sent");
    assert!(chat < records, "the chat the UI waits on comes first");
}

#[tokio::test]
async fn selecting_the_active_agent_again_sends_its_chat() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let second = fixture.second;
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, UiEvent::SelectAgent(second.to_hex()), &links).await;
    let _ = drain(&mut commands);

    // A UI that showed another agent meanwhile, without the session
    // following, asks for this one's chat again.
    handle_ui_event(&mut state, UiEvent::SelectAgent(second.to_hex()), &links).await;
    let again = drain(&mut commands);
    assert!(again
        .iter()
        .any(|(_, c)| matches!(c, UiCommand::LoadConversation(_))));

    handle_ui_event(&mut state, UiEvent::FocusAgent(second.to_hex()), &links).await;
    let focused = drain(&mut commands);
    assert!(!focused
        .iter()
        .any(|(_, c)| matches!(c, UiCommand::LoadConversation(_))));
}
