use super::output::{ExitReason, Output};
use super::run::{parse_session_id, session_id, Headless, Request, RunLimits, SessionChoice};
use super::tools::ToolFilter;
use super::OutputFormat;
use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::event_loop::{GateBase, TurnBase};
use crate::harness::{
    agent_tools, background_process_manager, build_hub, mode_policy, policy_tool_names, Harness,
    HubSetup,
};
use crate::long_run::LongRunSettings;
use crate::session_helpers::default_agent_config_with_auth;
use async_trait::async_trait;
use aura_core_types::AgentId;
use aura_model::{
    MockProvider, MockResponse, ModelProvider, ModelRequest, ModelResponse, ReasonerError,
};
use aura_store_db::RocksStore;
use aura_ui_terminal::{ApprovalLevel, PermissionMode};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// A mock model that also keeps every request it was sent.
struct Recording {
    inner: MockProvider,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

#[async_trait]
impl ModelProvider for Recording {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        self.requests.lock().unwrap().push(request.clone());
        self.inner.complete(request).await
    }

    async fn health_check(&self) -> bool {
        true
    }
}

/// stdout stand-in.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }

    fn json_lines(&self) -> Vec<Value> {
        self.text()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

struct Dirs {
    data: TempDir,
    workspace: TempDir,
}

fn dirs() -> Dirs {
    Dirs {
        data: TempDir::new().unwrap(),
        workspace: TempDir::new().unwrap(),
    }
}

/// A headless harness on `dirs` whose model answers with `provider`.
fn headless(
    dirs: &Dirs,
    provider: MockProvider,
    filter: ToolFilter,
) -> (Headless, Arc<Mutex<Vec<ModelRequest>>>) {
    let permissions = super::permissions(aura_core_modes::AgentMode::Agent, None);
    headless_in(dirs, provider, filter, permissions)
}

/// [`headless`] with its agents under `permissions`.
fn headless_in(
    dirs: &Dirs,
    provider: MockProvider,
    filter: ToolFilter,
    permissions: crate::event_loop::Permissions,
) -> (Headless, Arc<Mutex<Vec<ModelRequest>>>) {
    harness(dirs, provider, filter, permissions, GateBase::default())
}

/// [`headless_in`] with Gate classifying and routing as `gate` says.
fn harness(
    dirs: &Dirs,
    provider: MockProvider,
    filter: ToolFilter,
    permissions: crate::event_loop::Permissions,
    gate: GateBase,
) -> (Headless, Arc<Mutex<Vec<ModelRequest>>>) {
    let data = dirs.data.path();
    let workspace = dirs.workspace.path().to_path_buf();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(Recording {
        inner: provider,
        requests: Arc::clone(&requests),
    });
    let (tool_config, tools, web_tools) = agent_tools(false);
    let tool_names = policy_tool_names(&tools);
    let config = default_agent_config_with_auth("mock-model", None);
    let (commands, mut ui) = mpsc::channel(256);
    tokio::spawn(async move { while ui.recv().await.is_some() {} });
    let (process_manager, completions) = background_process_manager();
    let Harness { hub, background } = build_hub(HubSetup {
        data_dir: data.to_path_buf(),
        workspace_root: workspace.clone(),
        registry: AgentRegistry::load_or_init_in(data, &workspace).unwrap(),
        definitions: AgentDefinitions::load_or_seed(data).unwrap(),
        store: Arc::new(RocksStore::open(data.join("db"), false).unwrap()),
        provider,
        tool_config,
        base: TurnBase {
            limits: crate::model_selection::ModelLimits::of(&config),
            config,
            tools,
            gate,
        },
        // Runs never explore, so they go the same way every time.
        gate: aura_config::GateConfig {
            explore_rate: 0.0,
            ..aura_config::GateConfig::defaults()
        },
        policy: filter.apply(mode_policy(permissions, &tool_names), &tool_names),
        record_observer: None,
        record_panel: crate::event_loop::RecordPanelAgent::default(),
        record_router: None,
        trace_home: Some(data.join("home")),
        commands,
        long_run: LongRunSettings::default(),
        process_manager,
        background_agents: true,
        permissions,
        instructions: crate::project_instructions::ProjectInstructions::only_in(&workspace),
        approvals: None,
        web_tools,
        post_edit_check: aura_agent::PostEditCheckConfig::from_lookup(|_| None),
        trace_snapshots: false,
    });
    let notices = hub.take_notices();
    let headless = Headless {
        hub,
        background,
        completions,
        notices,
        model: "mock-model".to_string(),
        mode: permissions.mode.agent_mode(),
        workspace_root: workspace,
        tool_filter: filter,
        job_wait: Duration::from_secs(5),
        trace: None,
        notes: Vec::new(),
        judged: Vec::new(),
    };
    (headless, requests)
}

fn request(prompt: &str, session: SessionChoice) -> Request {
    Request {
        prompt: prompt.to_string(),
        session,
        limits: RunLimits::default(),
    }
}

async fn execute(
    headless: &mut Headless,
    request: Request,
    format: OutputFormat,
) -> (super::RunReport, Captured) {
    let captured = Captured::default();
    let output = Output::new(format, Box::new(captured.clone()));
    let report = tokio::time::timeout(
        Duration::from_secs(30),
        headless.execute(request, &output, &CancellationToken::new()),
    )
    .await
    .expect("headless run did not finish");
    output.finish(&report);
    (report, captured)
}

fn todo_call() -> MockResponse {
    MockResponse::tool_use(
        "t1",
        "todo_write",
        json!({"todos": [{"id": "1", "content": "Write the parser", "status": "completed"}]}),
    )
}

#[tokio::test]
async fn text_prints_only_the_final_reply() {
    let dirs = dirs();
    let (mut h, _) = headless(
        &dirs,
        MockProvider::simple_response("All done."),
        ToolFilter::default(),
    );

    let (report, out) = execute(
        &mut h,
        request("hi", SessionChoice::New),
        OutputFormat::Text,
    )
    .await;

    assert_eq!(report.exit_reason, ExitReason::Success);
    assert_eq!(out.text(), "All done.\n");
}

#[tokio::test]
async fn json_reports_usage_todos_and_session() {
    let dirs = dirs();
    let provider = MockProvider::new()
        .with_response(todo_call())
        .with_response(MockResponse::text("Finished."));
    let (mut h, _) = headless(&dirs, provider, ToolFilter::default());

    let (report, out) = execute(
        &mut h,
        request("build it", SessionChoice::New),
        OutputFormat::Json,
    )
    .await;

    let lines = out.json_lines();
    assert_eq!(lines.len(), 1, "{}", out.text());
    let json = &lines[0];
    assert_eq!(json["type"], "result");
    assert_eq!(json["exit_reason"], "success");
    assert_eq!(json["is_error"], false);
    assert_eq!(json["result"], "Finished.");
    assert_eq!(json["session_id"], report.session_id.as_str());
    assert!(report.session_id.contains(':'));
    assert_eq!(json["num_turns"], 2);
    assert_eq!(json["agent_turns"], 1);
    assert_eq!(json["usage"]["calls"], 2);
    assert!(json["usage"]["output_tokens"].as_u64().unwrap() > 0);
    assert!(json["usage"].get("cache_read_tokens").is_some());
    assert_eq!(json["compactions"], 0);
    assert_eq!(json["todos"][0]["content"], "Write the parser");
    assert_eq!(json["todos"][0]["status"], "completed");
}

#[tokio::test]
async fn stream_json_streams_events_and_ends_with_the_result() {
    let dirs = dirs();
    let provider = MockProvider::new()
        .with_response(todo_call())
        .with_response(MockResponse::text("Finished."));
    let (mut h, _) = headless(&dirs, provider, ToolFilter::default());

    let (_, out) = execute(
        &mut h,
        request("build it", SessionChoice::New),
        OutputFormat::StreamJson,
    )
    .await;

    let lines = out.json_lines();
    let types: Vec<&str> = lines.iter().map(|l| l["type"].as_str().unwrap()).collect();
    assert_eq!(types.first(), Some(&"init"), "{types:?}");
    assert_eq!(types.last(), Some(&"result"), "{types:?}");
    assert!(lines[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t == "todo_write"));
    let result = lines
        .iter()
        .find(|l| l["type"] == "tool_result")
        .expect("a tool_result line");
    assert_eq!(result["name"], "todo_write");
    assert_eq!(result["input"]["todos"][0]["id"], "1");
    assert_eq!(result["is_error"], false);
    assert!(types.contains(&"todos"), "{types:?}");
    assert_eq!(
        types.iter().filter(|t| **t == "usage").count(),
        2,
        "{}",
        out.text()
    );
    let streamed: String = lines
        .iter()
        .filter(|l| l["type"] == "text_delta")
        .filter_map(|l| l["text"].as_str())
        .collect();
    assert_eq!(streamed, "Finished.");
}

/// User prompts the model was sent in its last request.
fn last_prompts(requests: &Mutex<Vec<ModelRequest>>) -> Vec<String> {
    let requests = requests.lock().unwrap();
    requests
        .last()
        .unwrap()
        .messages
        .iter()
        .filter(|m| m.role == aura_model::Role::User)
        .map(aura_model::Message::text_content)
        .filter(|t| !t.is_empty())
        .collect()
}

#[tokio::test]
async fn resume_and_continue_pick_up_a_saved_session() {
    let dirs = dirs();
    let first = {
        let provider = MockProvider::new()
            .with_response(todo_call())
            .with_response(MockResponse::text("Noted."));
        let (mut h, _) = headless(&dirs, provider, ToolFilter::default());
        let (report, _) = execute(
            &mut h,
            request("remember 42", SessionChoice::New),
            OutputFormat::Json,
        )
        .await;
        report.session_id
    };

    // A new run starts over…
    {
        let (mut h, requests) = headless(
            &dirs,
            MockProvider::simple_response("Fresh."),
            ToolFilter::default(),
        );
        let (report, _) = execute(
            &mut h,
            request("unrelated", SessionChoice::New),
            OutputFormat::Json,
        )
        .await;
        assert_ne!(report.session_id, first);
        assert_eq!(last_prompts(&requests), ["unrelated"]);
        assert!(report.todos.is_empty());
    }

    // …and `--resume` brings the first one back, tool calls and todos
    // included.
    {
        let (mut h, requests) = headless(
            &dirs,
            MockProvider::simple_response("It was 42."),
            ToolFilter::default(),
        );
        let (report, _) = execute(
            &mut h,
            request("what was it?", SessionChoice::Resume(first.clone())),
            OutputFormat::Json,
        )
        .await;
        assert_eq!(report.exit_reason, ExitReason::Success);
        assert_eq!(report.session_id, first);
        assert_eq!(last_prompts(&requests), ["remember 42", "what was it?"]);
        assert_eq!(report.todos.len(), 1);
        let sent = &requests.lock().unwrap()[0];
        assert!(sent.messages.iter().any(aura_model::Message::has_tool_use));
    }

    // `--continue` keeps going in the session the last run used.
    {
        let (mut h, requests) = headless(
            &dirs,
            MockProvider::simple_response("Still 42."),
            ToolFilter::default(),
        );
        let (report, _) = execute(
            &mut h,
            request("and now?", SessionChoice::Continue),
            OutputFormat::Json,
        )
        .await;
        assert_eq!(report.session_id, first);
        assert_eq!(
            last_prompts(&requests),
            ["remember 42", "what was it?", "and now?"]
        );
    }
}

#[tokio::test]
async fn resuming_an_unknown_session_fails() {
    let dirs = dirs();
    let (mut h, _) = headless(
        &dirs,
        MockProvider::simple_response("x"),
        ToolFilter::default(),
    );
    let missing = session_id(AgentId::generate(), 1);

    let (report, out) = execute(
        &mut h,
        request("hi", SessionChoice::Resume(missing)),
        OutputFormat::Json,
    )
    .await;

    assert_eq!(report.exit_reason, ExitReason::Error);
    assert_eq!(report.exit_reason.exit_code(), 1);
    assert_eq!(out.json_lines()[0]["is_error"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_turns_stops_a_run_that_keeps_calling_tools() {
    let dirs = dirs();
    // Latency lets the limit land before the next call returns.
    let provider = MockProvider::new()
        .with_default_response(todo_call())
        .with_latency(50);
    let (mut h, _) = headless(&dirs, provider, ToolFilter::default());
    let mut req = request("loop", SessionChoice::New);
    req.limits.max_turns = Some(2);

    let (report, _) = execute(&mut h, req, OutputFormat::Json).await;

    assert_eq!(report.exit_reason, ExitReason::MaxTurns);
    assert_eq!(report.exit_reason.exit_code(), 3);
    assert!(report.is_error);
    assert_eq!(report.num_turns, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_time_stops_a_long_run_and_keeps_its_session() {
    let dirs = dirs();
    let provider = MockProvider::new()
        .with_default_response(todo_call())
        .with_latency(100);
    let (mut h, _) = headless(&dirs, provider, ToolFilter::default());
    let mut req = request("loop", SessionChoice::New);
    req.limits.max_time = Some(Duration::from_millis(400));

    let (report, _) = execute(&mut h, req, OutputFormat::Json).await;

    assert_eq!(report.exit_reason, ExitReason::MaxTime);
    assert!(report.num_turns >= 1);
    assert_eq!(report.todos.len(), 1, "the stopped turn's work was kept");
}

#[tokio::test]
async fn a_tool_outside_the_allow_list_is_denied() {
    let dirs = dirs();
    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "w1",
            "write_file",
            json!({"path": "out.txt", "content": "hi"}),
        ))
        .with_response(MockResponse::text("Tried."));
    let filter = ToolFilter::new(&["read_file".to_string()], &[]);
    let (mut h, requests) = headless(&dirs, provider, filter);

    let (report, out) = execute(
        &mut h,
        request("write", SessionChoice::New),
        OutputFormat::StreamJson,
    )
    .await;

    assert_eq!(report.exit_reason, ExitReason::Success);
    assert!(!Path::new(&dirs.workspace.path().join("out.txt")).exists());
    let lines = out.json_lines();
    let offered = lines[0]["tools"].as_array().unwrap();
    assert!(offered.iter().any(|t| t == "read_file"));
    assert!(!offered.iter().any(|t| t == "write_file"));
    let result = lines.iter().find(|l| l["type"] == "tool_result").unwrap();
    assert_eq!(result["is_error"], true, "{result}");
    let sent_tools: Vec<String> = requests.lock().unwrap()[0]
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert!(
        !sent_tools.iter().any(|t| t == "write_file"),
        "{sent_tools:?}"
    );
}

#[tokio::test]
async fn web_tools_can_be_disallowed() {
    let dirs = dirs();
    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "f1",
            "web_fetch",
            json!({"url": "https://example.com"}),
        ))
        .with_response(MockResponse::text("Tried."));
    let filter = ToolFilter::new(&[], &["web_fetch,web_search".to_string()]);
    let (mut h, _) = headless(&dirs, provider, filter);

    let (report, out) = execute(
        &mut h,
        request("fetch", SessionChoice::New),
        OutputFormat::StreamJson,
    )
    .await;

    assert_eq!(report.exit_reason, ExitReason::Success);
    let lines = out.json_lines();
    let offered = lines[0]["tools"].as_array().unwrap();
    assert!(offered.iter().any(|t| t == "read_file"), "{offered:?}");
    assert!(!offered
        .iter()
        .any(|t| t == "web_fetch" || t == "web_search"));
    let result = lines.iter().find(|l| l["type"] == "tool_result").unwrap();
    assert_eq!(result["is_error"], true, "{result}");
}

fn write_out_txt() -> MockProvider {
    MockProvider::new()
        .with_response(MockResponse::tool_use(
            "w1",
            "write_file",
            json!({"path": "out.txt", "content": "hi"}),
        ))
        .with_response(MockResponse::text("Wrote it."))
}

#[tokio::test]
async fn agent_mode_runs_edits_without_asking() {
    let dirs = dirs();
    let (mut h, _) = headless(&dirs, write_out_txt(), ToolFilter::default());

    let (report, _) = execute(
        &mut h,
        request("write", SessionChoice::New),
        OutputFormat::Json,
    )
    .await;

    assert_eq!(report.exit_reason, ExitReason::Success);
    assert!(dirs.workspace.path().join("out.txt").exists());
}

#[tokio::test]
async fn a_call_the_mode_asks_about_is_denied() {
    let dirs = dirs();
    let (mut h, _) = headless_in(
        &dirs,
        write_out_txt(),
        ToolFilter::default(),
        crate::event_loop::Permissions::new(PermissionMode::Agent, ApprovalLevel::Ask),
    );

    let (report, out) = execute(
        &mut h,
        request("write", SessionChoice::New),
        OutputFormat::StreamJson,
    )
    .await;

    assert_eq!(report.exit_reason, ExitReason::Success);
    assert!(!dirs.workspace.path().join("out.txt").exists());
    let lines = out.json_lines();
    let result = lines.iter().find(|l| l["type"] == "tool_result").unwrap();
    assert_eq!(result["is_error"], true, "{result}");
}

#[test]
fn headless_runs_agent_mode_at_yolo_unless_told_otherwise() {
    use aura_core_modes::AgentMode;
    let agent = super::permissions(AgentMode::Agent, None);
    assert_eq!(agent.mode, PermissionMode::Agent);
    assert_eq!(agent.approval, ApprovalLevel::Yolo);
    for mode in [AgentMode::Plan, AgentMode::Ask, AgentMode::Debug] {
        let permissions = super::permissions(mode, None);
        assert_eq!(permissions.mode, PermissionMode::from_agent_mode(mode));
        assert_eq!(permissions.mode.agent_mode(), mode);
        assert_eq!(permissions.approval, ApprovalLevel::Ask);
    }
    assert_eq!(
        super::permissions(AgentMode::Agent, Some(ApprovalLevel::Strict)).approval,
        ApprovalLevel::Strict
    );
}

#[test]
fn session_ids_round_trip() {
    let agent = AgentId::generate();
    let id = session_id(agent, 1_700_000_000_000);
    assert_eq!(
        parse_session_id(&id, AgentId::generate()),
        Ok((agent, 1_700_000_000_000))
    );
    let other = AgentId::generate();
    assert_eq!(parse_session_id(" 12 ", other), Ok((other, 12)));
    assert!(parse_session_id("nope", other).is_err());
    assert!(parse_session_id("zz:12", other).is_err());
}

#[tokio::test]
async fn gate_routes_headless_turns_and_records_how_they_went() {
    use aura_agent_gate::Candidate;
    use aura_core_types::{OutcomeSignal, PromptCategory, Route, RouteReason};
    use aura_model::model_catalog::{ModelBackend, ModelTier};
    const HAIKU: &str = "aura-claude-haiku-4-5";
    const SONNET: &str = "aura-claude-sonnet-4-6";
    let candidate = |id: &str, tier, cost_usd| Candidate {
        id: id.to_string(),
        label: id.to_string(),
        backend: ModelBackend::Router,
        tier,
        cost_usd: Some(cost_usd),
    };
    // One candidate, so the run never explores.
    let gate = GateBase {
        classifier: HAIKU.to_string(),
        candidates: Some(vec![candidate(SONNET, ModelTier::Standard, 0.1)]),
    };
    let dirs = dirs();
    let permissions = super::permissions(aura_core_modes::AgentMode::Agent, None);
    let (mut h, requests) = harness(
        &dirs,
        MockProvider::simple_response("question"),
        ToolFilter::default(),
        permissions,
        gate,
    );

    let (report, _) = execute(
        &mut h,
        request("what is a monad?", SessionChoice::New),
        OutputFormat::Json,
    )
    .await;

    assert_eq!(report.exit_reason, ExitReason::Success);
    assert_eq!(report.model, SONNET);
    let models: Vec<String> = requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.model.to_string())
        .collect();
    assert_eq!(models, [HAIKU, SONNET], "classified, then routed");
    {
        let requests = requests.lock().unwrap();
        let turn = &requests[1];
        let tools: Vec<&str> = turn.tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            tools.contains(&"write_file") && tools.contains(&"edit_file"),
            "a question is answered by the model it is routed to: {tools:?}"
        );
        assert!(!turn.system.contains("<gate_lead>"));
    }
    assert_eq!(
        h.setup_lines(),
        [
            format!("gate: prompts classified by {HAIKU} (cloud)"),
            format!("gate: routing among {SONNET} (cloud)"),
        ]
    );
    let (agent, _) = parse_session_id(&report.session_id, AgentId::generate()).unwrap();
    let entries = h.hub.factory.store.scan_record(agent, 1, 1_000).unwrap();
    let prompts: Vec<_> = entries
        .iter()
        .filter_map(|e| e.tx.prompt_payload())
        .collect();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].category, PromptCategory::Question);
    assert_eq!(prompts[0].classifier, HAIKU);
    assert_eq!(prompts[0].route, Route::turn(SONNET, RouteReason::Default),);
    let outcomes: Vec<_> = entries
        .iter()
        .filter_map(|e| e.tx.outcome_payload())
        .collect();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].model, SONNET);
    assert_eq!(outcomes[0].signal, OutcomeSignal::None);
    assert_eq!(h.hub.gate.recent().len(), 1);
}

#[tokio::test]
async fn a_headless_turn_changing_several_files_is_led() {
    use aura_agent_gate::Candidate;
    use aura_model::model_catalog::{ModelBackend, ModelTier};
    const HAIKU: &str = "aura-claude-haiku-4-5";
    const SONNET: &str = "aura-claude-sonnet-4-6";
    let gate = GateBase {
        classifier: HAIKU.to_string(),
        candidates: Some(vec![Candidate {
            id: SONNET.to_string(),
            label: SONNET.to_string(),
            backend: ModelBackend::Router,
            tier: ModelTier::Standard,
            cost_usd: Some(0.1),
        }]),
    };
    let dirs = dirs();
    let permissions = super::permissions(aura_core_modes::AgentMode::Agent, None);
    let (mut h, requests) = harness(
        &dirs,
        MockProvider::simple_response("multi_file_change"),
        ToolFilter::default(),
        permissions,
        gate,
    );
    for (led, outcomes) in [(true, (10, true)), (false, (3, false))] {
        h.hub.gate.learn(
            aura_core_types::RouteLevel::Turn,
            aura_core_types::PromptCategory::MultiFileChange,
            SONNET,
            Some(led),
            outcomes,
        );
    }

    let (report, _) = execute(
        &mut h,
        request("rename the crate everywhere", SessionChoice::New),
        OutputFormat::Json,
    )
    .await;

    assert_eq!(report.exit_reason, ExitReason::Success);
    let requests = requests.lock().unwrap();
    let lead = &requests[1];
    let tools: Vec<&str> = lead.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(tools.contains(&"read_file") && tools.contains(&"task"));
    assert!(
        !tools.contains(&"write_file") && !tools.contains(&"edit_file"),
        "the headless lead delegates its edits: {tools:?}"
    );
    assert!(lead.system.contains("<gate_lead>"));
}

#[tokio::test]
async fn an_eval_run_under_gate_is_routed_and_its_verdict_feeds_the_table() {
    use super::{EvalTurn, EvalTurns};
    use crate::event_loop::subagents::lock;
    use aura_agent_gate::Candidate;
    use aura_agent_trace::{RunBundle, RunMeta, RunRecorder, RunSource, RunTrace};
    use aura_core_types::{
        OutcomeSignal, PromptCategory, Route, RouteLevel, RouteReason, ToolProposal,
        TransactionType,
    };
    use aura_model::model_catalog::{ModelBackend, ModelTier};
    const HAIKU: &str = "aura-claude-haiku-4-5";
    const SONNET: &str = "aura-claude-sonnet-4-6";
    // One candidate, so the runs never explore.
    let gate = GateBase {
        classifier: HAIKU.to_string(),
        candidates: Some(vec![Candidate {
            id: SONNET.to_string(),
            label: SONNET.to_string(),
            backend: ModelBackend::Router,
            tier: ModelTier::Standard,
            cost_usd: Some(0.1),
        }]),
    };
    let dirs = dirs();
    let permissions = super::permissions(aura_core_modes::AgentMode::Agent, None);
    let (h, requests) = harness(
        &dirs,
        MockProvider::new().with_default_response(MockResponse::text("question")),
        ToolFilter::default(),
        permissions,
        gate,
    );
    let hub = Arc::clone(&h.hub);
    let mut turns = EvalTurns::new(h);
    let home = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    let tasks = [
        ("fails", "is there a missing.txt?", "test -f missing.txt"),
        (
            "passes",
            "what does note.txt say?",
            "[ \"$(cat note.txt)\" = 'hi' ]",
        ),
    ];

    let mut runs = Vec::new();
    for (id, prompt, verify) in tasks {
        let workspace = work.path().join(id);
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("note.txt"), "hi\n").unwrap();
        let workspace = workspace.canonicalize().unwrap();
        let mut meta = RunMeta::new("gate", RunSource::Eval);
        meta.task_id = Some(format!("s/{id}"));
        let trace = RunRecorder::start(home.path(), meta, aura_config::trace::TraceLevel::Full)
            .unwrap()
            .unwrap();
        let ran = tokio::time::timeout(
            Duration::from_secs(60),
            turns.run(EvalTurn {
                name: &format!("Eval s/{id}"),
                workspace: &workspace,
                prompts: &[prompt.to_string()],
                max_iterations: None,
                checks: None,
                verify,
                trace: Arc::clone(&trace),
            }),
        )
        .await
        .expect("the eval run did not finish")
        .unwrap();
        let card = trace.finish().unwrap();
        runs.push((ran, trace.run_id(), card));
    }

    let passed: Vec<Option<bool>> = runs.iter().map(|(ran, _, _)| ran.check.passed).collect();
    assert_eq!(passed, [Some(false), Some(true)]);
    let models: Vec<String> = requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.model.to_string())
        .collect();
    assert_eq!(
        models,
        [HAIKU, SONNET, HAIKU, SONNET],
        "each task classified, then routed"
    );
    for (ran, _, _) in &runs {
        assert_eq!(ran.gate.routed, [SONNET]);
        let calls: u32 = ran.gate.tokens.iter().map(|m| m.calls).sum();
        assert_eq!(calls, 2, "the classifier call and the turn");
        assert!(ran.error.is_none(), "{:?}", ran.error);
    }

    let agents: Vec<_> = lock(&hub.registry)
        .agents()
        .iter()
        .filter(|a| a.name.starts_with("Eval s/"))
        .cloned()
        .collect();
    assert_eq!(agents.len(), 2);
    for (agent, verdict) in agents
        .iter()
        .zip([OutcomeSignal::ChecksFailed, OutcomeSignal::ChecksPassed])
    {
        let entries = hub
            .factory
            .store
            .scan_record(agent.agent_id(), 1, 1_000)
            .unwrap();
        let prompts: Vec<_> = entries
            .iter()
            .filter_map(|e| e.tx.prompt_payload())
            .collect();
        assert_eq!(prompts.len(), 1, "{}", agent.name);
        assert_eq!(prompts[0].category, PromptCategory::Question);
        assert_eq!(prompts[0].route, Route::turn(SONNET, RouteReason::Default));
        let signals: Vec<OutcomeSignal> = entries
            .iter()
            .filter_map(|e| e.tx.outcome_payload())
            .map(|o| o.signal)
            .collect();
        assert_eq!(signals, [OutcomeSignal::None, verdict], "{}", agent.name);
        let verify = entries
            .iter()
            .filter(|e| e.tx.tx_type == TransactionType::ToolProposal)
            .filter_map(|e| serde_json::from_slice::<ToolProposal>(&e.tx.payload).ok())
            .find(|p| p.source.as_deref() == Some("eval_verify"))
            .expect("the verify command went through the agent's kernel");
        assert_eq!(verify.tool, "run_command");
    }
    let row = hub
        .gate
        .stats()
        .row(RouteLevel::Turn, PromptCategory::Question, SONNET)
        .cloned()
        .expect("the eval runs count in Gate's table");
    assert_eq!((row.tries, row.successes), (2, 1));

    let (_, run_id, card) = &runs[1];
    let trace = RunTrace::load(&RunBundle::find(home.path(), run_id).unwrap()).unwrap();
    assert_eq!(trace.meta.model, "gate");
    assert_eq!(card.model_calls, 1, "the routed turn is in the run's trace");
}

/// What a run whose first turn ends with a failing check left behind.
struct FailedCheckRun {
    report: super::RunReport,
    lines: Vec<Value>,
    /// Model of each model call, in order
    models: Vec<String>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    routes: Vec<(aura_core_types::PromptCategory, aura_core_types::Route)>,
    outcomes: Vec<(String, aura_core_types::OutcomeSignal)>,
    /// Tries and successes of the debugging rows of Sonnet and Opus
    rows: [Option<(u32, u32)>; 2],
}

const GATE_HAIKU: &str = "aura-claude-haiku-4-5";
const GATE_SONNET: &str = "aura-claude-sonnet-4-6";
const GATE_OPUS: &str = "aura-claude-opus-4-7";

/// Gate routing among Sonnet and Opus, classifying with Haiku. Debugging
/// starts on Sonnet, the cheapest of the two, so runs never explore.
fn sonnet_and_opus() -> GateBase {
    use aura_agent_gate::Candidate;
    use aura_model::model_catalog::{ModelBackend, ModelTier};
    let candidate = |id: &str, tier, cost_usd| Candidate {
        id: id.to_string(),
        label: id.to_string(),
        backend: ModelBackend::Router,
        tier,
        cost_usd: Some(cost_usd),
    };
    GateBase {
        classifier: GATE_HAIKU.to_string(),
        candidates: Some(vec![
            candidate(GATE_SONNET, ModelTier::Standard, 0.1),
            candidate(GATE_OPUS, ModelTier::Frontier, 0.2),
        ]),
    }
}

/// A model whose prompt is classified as debugging and whose turn writes
/// `parser.txt`, then runs `cargo test` (which fails in a workspace with
/// no crate) and gives up. The model calls after that are answered with
/// `then`, in order.
fn failing_check_provider(then: &[&str]) -> MockProvider {
    let mut provider = MockProvider::new()
        .with_response(MockResponse::text("debugging"))
        .with_response(MockResponse::tool_use(
            "c0",
            "write_file",
            json!({"path": "parser.txt", "content": "skip the last token\n"}),
        ))
        .with_response(MockResponse::tool_use(
            "c1",
            "run_command",
            json!({"program": "cargo", "args": ["test"]}),
        ))
        .with_response(MockResponse::text("The tests still fail."));
    for reply in then {
        provider = provider.with_response(MockResponse::text(*reply));
    }
    provider
}

/// Run "fix the parser" under [`sonnet_and_opus`] on
/// [`failing_check_provider`]: the turn starts on Sonnet and its last
/// check fails.
async fn run_with_a_failing_check(then: &[&str]) -> FailedCheckRun {
    use aura_core_types::{PromptCategory, RouteLevel};
    let gate = sonnet_and_opus();
    let dirs = dirs();
    let permissions = super::permissions(aura_core_modes::AgentMode::Agent, None);
    let provider = failing_check_provider(then);
    let (mut h, requests) = harness(&dirs, provider, ToolFilter::default(), permissions, gate);

    let (report, captured) = execute(
        &mut h,
        request("fix the parser", SessionChoice::New),
        OutputFormat::StreamJson,
    )
    .await;

    let models = requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.model.to_string())
        .collect();
    let (agent, _) = parse_session_id(&report.session_id, AgentId::generate()).unwrap();
    let entries = h.hub.factory.store.scan_record(agent, 1, 1_000).unwrap();
    let stats = h.hub.gate.stats();
    let row = |model: &str| {
        stats
            .row(RouteLevel::Turn, PromptCategory::Debugging, model)
            .map(|row| (row.tries, row.successes))
    };
    FailedCheckRun {
        lines: captured.json_lines(),
        models,
        requests,
        routes: entries
            .iter()
            .filter_map(|e| e.tx.prompt_payload())
            .map(|p| (p.category, p.route))
            .collect(),
        outcomes: entries
            .iter()
            .filter_map(|e| e.tx.outcome_payload())
            .map(|o| (o.model, o.signal))
            .collect(),
        rows: [row(GATE_SONNET), row(GATE_OPUS)],
        report,
    }
}

#[tokio::test]
async fn a_failed_turn_the_judge_blames_on_the_model_goes_on_with_a_stronger_one() {
    use aura_core_types::{OutcomeSignal, PromptCategory, Route, RouteReason};
    let run = run_with_a_failing_check(&[
        "The parser still drops the last token.\nVERDICT: MODEL",
        "Fixed it.",
    ])
    .await;

    let report = &run.report;
    assert_eq!(
        report.exit_reason,
        ExitReason::Success,
        "{:?}",
        report.error
    );
    assert_eq!(report.model, GATE_OPUS);
    assert_eq!(report.result, "Fixed it.");
    assert_eq!(report.agent_turns, 2);
    assert_eq!(
        run.models,
        [
            GATE_HAIKU,
            GATE_SONNET,
            GATE_SONNET,
            GATE_SONNET,
            GATE_OPUS,
            GATE_OPUS
        ],
        "classified, worked on Sonnet, judged by Opus, then moved up a tier"
    );
    {
        let requests = run.requests.lock().unwrap();
        let judge = &requests[4];
        assert_eq!(
            judge.system.as_str(),
            aura_agent_gate::turn_judge::SYSTEM_PROMPT
        );
        assert!(judge.tools.is_empty());
        let shown = judge.messages[0].text_content();
        assert!(shown.contains("fix the parser"), "{shown}");
        assert!(shown.contains("`cargo test`, failed"), "{shown}");
        assert!(
            shown.contains("=== parser.txt") && shown.contains("+skip the last token"),
            "the judge sees what the turn changed: {shown}"
        );
        assert!(shown.contains("The tests still fail."), "{shown}");
    }
    let takeover = last_prompts(&run.requests).pop().unwrap();
    assert!(
        takeover.contains("You are a stronger model taking over"),
        "the stronger model carries on in the same conversation: {takeover}"
    );
    assert!(
        takeover.contains("The parser still drops the last token."),
        "it is told what the judge found: {takeover}"
    );
    let judged: Vec<&Value> = run
        .lines
        .iter()
        .filter(|line| line["type"] == "judged")
        .collect();
    assert_eq!(judged.len(), 1, "{:?}", run.lines);
    assert_eq!(judged[0]["model"], GATE_SONNET);
    assert_eq!(judged[0]["judge"], GATE_OPUS);
    assert_eq!(judged[0]["at_fault"], true);

    assert_eq!(
        run.routes,
        [
            (
                PromptCategory::Debugging,
                Route::turn(GATE_SONNET, RouteReason::Default)
            ),
            (
                PromptCategory::Debugging,
                Route::turn(GATE_OPUS, RouteReason::Escalate)
            ),
        ]
    );
    assert_eq!(
        run.outcomes,
        [
            (GATE_SONNET.to_string(), OutcomeSignal::ChecksFailed),
            (GATE_SONNET.to_string(), OutcomeSignal::JudgedFail),
            (GATE_OPUS.to_string(), OutcomeSignal::None),
        ]
    );
    assert_eq!(
        run.rows,
        [Some((1, 0)), None],
        "the failure counts once for Sonnet; the stronger model's turn is not counted"
    );
}

#[tokio::test]
async fn an_eval_verdict_after_a_move_up_lands_on_the_uncounted_attempt() {
    use super::{EvalTurn, EvalTurns};
    use aura_agent_trace::{RunMeta, RunRecorder, RunSource};
    use aura_core_types::{OutcomeSignal, PromptCategory, RouteLevel};
    let dirs = dirs();
    let permissions = super::permissions(aura_core_modes::AgentMode::Agent, None);
    let provider = failing_check_provider(&["It skips a token.\nVERDICT: MODEL", "Fixed it."]);
    let (h, requests) = harness(
        &dirs,
        provider,
        ToolFilter::default(),
        permissions,
        sonnet_and_opus(),
    );
    let hub = Arc::clone(&h.hub);
    let mut turns = EvalTurns::new(h);
    let home = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    let workspace = work.path().canonicalize().unwrap();
    let mut meta = RunMeta::new("gate", RunSource::Eval);
    meta.task_id = Some("s/parser".to_string());
    let trace = RunRecorder::start(home.path(), meta, aura_config::trace::TraceLevel::Full)
        .unwrap()
        .unwrap();

    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        turns.run(EvalTurn {
            name: "Eval s/parser",
            workspace: &workspace,
            prompts: &["fix the parser".to_string()],
            max_iterations: None,
            checks: None,
            verify: "test -f parser.txt",
            trace: Arc::clone(&trace),
        }),
    )
    .await
    .expect("the eval run did not finish")
    .unwrap();

    assert!(ran.error.is_none(), "{:?}", ran.error);
    assert_eq!(ran.check.passed, Some(true));
    assert_eq!(ran.gate.routed, [GATE_SONNET, GATE_OPUS]);
    assert_eq!(ran.gate.judged.len(), 1);
    let judged = &ran.gate.judged[0];
    assert_eq!(
        (judged.model.as_str(), judged.judge.as_str()),
        (GATE_SONNET, GATE_OPUS)
    );
    assert!(judged.verdict.at_fault);
    assert_eq!(judged.verdict.reasons, "It skips a token.");
    let asked: Vec<String> = requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.model.to_string())
        .collect();
    let recorded: u32 = ran.gate.tokens.iter().map(|m| m.calls).sum();
    assert_eq!(
        recorded as usize,
        asked.len(),
        "every model call is in the run's cost, the judge's included: {asked:?}"
    );
    assert_eq!(
        asked.iter().filter(|model| *model == GATE_OPUS).count(),
        2,
        "the judge's call and the turn it sent on: {asked:?}"
    );

    let agent = crate::event_loop::subagents::lock(&hub.registry)
        .agents()
        .iter()
        .find(|a| a.name == "Eval s/parser")
        .unwrap()
        .agent_id();
    let outcomes: Vec<(String, OutcomeSignal)> = hub
        .factory
        .store
        .scan_record(agent, 1, 1_000)
        .unwrap()
        .iter()
        .filter_map(|e| e.tx.outcome_payload())
        .map(|o| (o.model, o.signal))
        .collect();
    assert_eq!(
        outcomes,
        [
            (GATE_SONNET.to_string(), OutcomeSignal::ChecksFailed),
            (GATE_SONNET.to_string(), OutcomeSignal::JudgedFail),
            (GATE_OPUS.to_string(), OutcomeSignal::None),
            (GATE_OPUS.to_string(), OutcomeSignal::ChecksPassed),
        ],
        "the hidden check's verdict is recorded for the model that finished the work"
    );
    let stats = hub.gate.stats();
    let row = |model: &str| {
        stats
            .row(RouteLevel::Turn, PromptCategory::Debugging, model)
            .map(|row| (row.tries, row.successes))
    };
    assert_eq!(
        row(GATE_SONNET),
        Some((1, 0)),
        "the start model's failure stands"
    );
    assert_eq!(row(GATE_OPUS), None, "work taken over is not counted");
}

#[tokio::test]
async fn a_failed_turn_the_judge_excuses_stays_put_and_counts_for_nothing() {
    use aura_core_types::{OutcomeSignal, PromptCategory, Route, RouteReason};
    let run = run_with_a_failing_check(&["There is no crate here to test.\nVERDICT: OTHER"]).await;

    let report = &run.report;
    assert_eq!(
        report.exit_reason,
        ExitReason::Success,
        "{:?}",
        report.error
    );
    assert_eq!(report.model, GATE_SONNET);
    assert_eq!(report.result, "The tests still fail.");
    assert_eq!(report.agent_turns, 1, "no stronger model takes over");
    assert_eq!(
        run.models,
        [GATE_HAIKU, GATE_SONNET, GATE_SONNET, GATE_SONNET, GATE_OPUS],
        "the judge's call is the only one on Opus"
    );
    let judged: Vec<&Value> = run
        .lines
        .iter()
        .filter(|line| line["type"] == "judged")
        .collect();
    assert_eq!(judged.len(), 1, "{:?}", run.lines);
    assert_eq!(judged[0]["at_fault"], false);
    assert_eq!(judged[0]["reasons"], "There is no crate here to test.");
    assert_eq!(
        run.routes,
        [(
            PromptCategory::Debugging,
            Route::turn(GATE_SONNET, RouteReason::Default)
        )]
    );
    assert_eq!(
        run.outcomes,
        [
            (GATE_SONNET.to_string(), OutcomeSignal::ChecksFailed),
            (GATE_SONNET.to_string(), OutcomeSignal::JudgedExcused),
        ]
    );
    assert_eq!(
        run.rows,
        [None, None],
        "a failure that was not the model's doing counts for nothing"
    );
}

#[tokio::test]
async fn a_failed_turn_the_judge_says_nothing_clear_about_still_moves_up() {
    use aura_core_types::OutcomeSignal;
    let run = run_with_a_failing_check(&["Hard to say.", "Fixed it."]).await;

    assert_eq!(run.report.model, GATE_OPUS);
    assert_eq!(run.report.result, "Fixed it.");
    assert!(run.lines.iter().all(|line| line["type"] != "judged"));
    assert_eq!(
        run.outcomes,
        [
            (GATE_SONNET.to_string(), OutcomeSignal::ChecksFailed),
            (GATE_OPUS.to_string(), OutcomeSignal::None),
        ],
        "the turn's own signal stands"
    );
    assert_eq!(run.rows, [Some((1, 0)), None]);
}

/// Run "fix the parser" under [`sonnet_and_opus`] on `provider`, which
/// answers the classifier first. Returns the report, the model of each
/// call, the requests, and the model and signal of each `Outcome`.
async fn run_routed(
    provider: MockProvider,
) -> (
    super::RunReport,
    Vec<String>,
    Arc<Mutex<Vec<ModelRequest>>>,
    Vec<aura_core_types::OutcomePayload>,
) {
    let dirs = dirs();
    let permissions = super::permissions(aura_core_modes::AgentMode::Agent, None);
    let (mut h, requests) = harness(
        &dirs,
        provider,
        ToolFilter::default(),
        permissions,
        sonnet_and_opus(),
    );
    let (report, _) = execute(
        &mut h,
        request("fix the parser", SessionChoice::New),
        OutputFormat::StreamJson,
    )
    .await;
    let models = requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.model.to_string())
        .collect();
    let (agent, _) = parse_session_id(&report.session_id, AgentId::generate()).unwrap();
    let outcomes = h
        .hub
        .factory
        .store
        .scan_record(agent, 1, 1_000)
        .unwrap()
        .iter()
        .filter_map(|e| e.tx.outcome_payload())
        .collect();
    (report, models, requests, outcomes)
}

#[tokio::test]
async fn a_change_made_without_a_check_is_checked_before_the_turn_ends() {
    use aura_core_types::OutcomeSignal;
    let provider = MockProvider::new()
        .with_response(MockResponse::text("debugging"))
        .with_response(MockResponse::tool_use(
            "c0",
            "write_file",
            json!({"path": "parser.txt", "content": "skip the last token\n"}),
        ))
        .with_response(MockResponse::text("Fixed the parser."))
        .with_response(MockResponse::tool_use(
            "c1",
            "run_command",
            json!({"program": "cargo", "args": ["test"]}),
        ))
        .with_response(MockResponse::text("The tests fail."))
        .with_response(MockResponse::text(
            "The parser still drops the last token.\nVERDICT: MODEL",
        ))
        .with_response(MockResponse::text("Fixed it."));
    let (report, models, requests, outcomes) = run_routed(provider).await;

    assert_eq!(
        report.exit_reason,
        ExitReason::Success,
        "{:?}",
        report.error
    );
    assert_eq!(
        (report.model.as_str(), report.result.as_str()),
        (GATE_OPUS, "Fixed it.")
    );
    assert_eq!(
        report.agent_turns, 3,
        "the turn, its check, the stronger model"
    );
    assert_eq!(
        models,
        [
            GATE_HAIKU,
            GATE_SONNET,
            GATE_SONNET,
            GATE_SONNET,
            GATE_SONNET,
            GATE_OPUS,
            GATE_OPUS
        ],
        "Sonnet is asked to check its change, its check fails, the judge blames it, Opus takes over"
    );
    let nudge = requests.lock().unwrap()[3]
        .messages
        .last()
        .unwrap()
        .text_content();
    assert!(
        nudge.contains(aura_agent_gate::followup::NUDGE_PROMPT),
        "{nudge}"
    );
    let signals: Vec<(&str, OutcomeSignal)> = outcomes
        .iter()
        .map(|o| (o.model.as_str(), o.signal))
        .collect();
    assert_eq!(
        signals,
        [
            (GATE_SONNET, OutcomeSignal::None),
            (GATE_SONNET, OutcomeSignal::ChecksFailed),
            (GATE_SONNET, OutcomeSignal::JudgedFail),
            (GATE_OPUS, OutcomeSignal::None),
        ]
    );
    assert_eq!(
        outcomes[1].prompt_tx, outcomes[0].prompt_tx,
        "the check speaks for the turn that made the change"
    );
    assert!(
        outcomes[1].cost_usd >= outcomes[0].cost_usd,
        "and its cost includes the turn's"
    );
}

#[tokio::test]
async fn unchecked_work_that_looks_unfinished_goes_to_the_judge_then_moves_up() {
    use aura_core_types::OutcomeSignal;
    let provider = MockProvider::new()
        .with_response(MockResponse::text("debugging"))
        .with_response(MockResponse::text(
            "I could not find where the parser drops the token.",
        ))
        .with_response(MockResponse::text(
            "It never opened the parser.\nVERDICT: MODEL",
        ))
        .with_response(MockResponse::text("Fixed it."));
    let (report, models, requests, outcomes) = run_routed(provider).await;

    assert_eq!(
        (report.model.as_str(), report.result.as_str()),
        (GATE_OPUS, "Fixed it.")
    );
    assert_eq!(models, [GATE_HAIKU, GATE_SONNET, GATE_OPUS, GATE_OPUS]);
    {
        let requests = requests.lock().unwrap();
        let judge = &requests[2];
        assert_eq!(
            judge.system.as_str(),
            aura_agent_gate::turn_judge::UNVERIFIED_SYSTEM_PROMPT
        );
        let shown = judge.messages[0].text_content();
        assert!(shown.contains("fix the parser"), "{shown}");
        assert!(shown.contains("I could not find"), "{shown}");
    }
    let takeover = last_prompts(&requests).pop().unwrap();
    assert!(
        takeover.contains("It never opened the parser."),
        "the stronger model is told what the judge found: {takeover}"
    );
    let signals: Vec<(&str, OutcomeSignal)> = outcomes
        .iter()
        .map(|o| (o.model.as_str(), o.signal))
        .collect();
    assert_eq!(
        signals,
        [
            (GATE_SONNET, OutcomeSignal::None),
            (GATE_SONNET, OutcomeSignal::JudgedFail),
            (GATE_OPUS, OutcomeSignal::None),
        ]
    );
}
