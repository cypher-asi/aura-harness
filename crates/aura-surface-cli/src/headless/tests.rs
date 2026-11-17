use super::output::{ExitReason, Output};
use super::run::{parse_session_id, session_id, Headless, Request, RunLimits, SessionChoice};
use super::tools::ToolFilter;
use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::cli::OutputFormat;
use crate::event_loop::{GateBase, TurnBase};
use crate::long_run::LongRunSettings;
use crate::runner::{
    agent_tools, background_process_manager, build_hub, mode_policy, policy_tool_names, Harness,
    HubSetup,
};
use crate::session_helpers::default_agent_config_with_auth;
use async_trait::async_trait;
use aura_core_types::AgentId;
use aura_model_reasoner::{
    MockProvider, MockResponse, ModelProvider, ModelRequest, ModelResponse, ReasonerError,
};
use aura_store_db::RocksStore;
use aura_terminal::PermissionMode;
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
    let mode = super::permission_mode(aura_core_modes::AgentMode::Agent);
    headless_in(dirs, provider, filter, mode)
}

/// [`headless`] with its agents in permission `mode`.
fn headless_in(
    dirs: &Dirs,
    provider: MockProvider,
    filter: ToolFilter,
    mode: PermissionMode,
) -> (Headless, Arc<Mutex<Vec<ModelRequest>>>) {
    harness(dirs, provider, filter, mode, GateBase::default())
}

/// [`headless_in`] with Gate classifying and routing as `gate` says.
fn harness(
    dirs: &Dirs,
    provider: MockProvider,
    filter: ToolFilter,
    mode: PermissionMode,
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
        registry: AgentRegistry::load_or_init(data).unwrap(),
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
        policy: filter.apply(mode_policy(mode, &tool_names), &tool_names),
        record_observer: None,
        record_panel: crate::event_loop::RecordPanelAgent::default(),
        commands,
        long_run: LongRunSettings::default(),
        process_manager,
        background_agents: true,
        mode,
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
        mode: mode.agent_mode(),
        workspace_root: workspace,
        tool_filter: filter,
        job_wait: Duration::from_secs(5),
        trace: None,
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
        .filter(|m| m.role == aura_model_reasoner::Role::User)
        .map(aura_model_reasoner::Message::text_content)
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
        assert!(sent
            .messages
            .iter()
            .any(aura_model_reasoner::Message::has_tool_use));
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
        PermissionMode::Default,
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
fn headless_modes_match_the_terminal_except_agent() {
    use aura_core_modes::AgentMode;
    assert_eq!(
        super::permission_mode(AgentMode::Agent),
        PermissionMode::Unrestricted
    );
    for mode in [AgentMode::Plan, AgentMode::Ask, AgentMode::Debug] {
        assert_eq!(
            super::permission_mode(mode),
            PermissionMode::from_agent_mode(mode)
        );
        assert_eq!(super::permission_mode(mode).agent_mode(), mode);
    }
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
    use aura_model_reasoner::model_catalog::{ModelBackend, ModelTier};
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
    let mode = super::permission_mode(aura_core_modes::AgentMode::Agent);
    let (mut h, requests) = harness(
        &dirs,
        MockProvider::simple_response("question"),
        ToolFilter::default(),
        mode,
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
        let lead = &requests[1];
        let tools: Vec<&str> = lead.tools.iter().map(|t| t.name.as_str()).collect();
        assert!(tools.contains(&"read_file") && tools.contains(&"run_command"));
        assert!(
            !tools.contains(&"write_file") && !tools.contains(&"edit_file"),
            "the headless lead delegates its edits: {tools:?}"
        );
        assert!(lead.system.contains("<gate_lead>"));
    }
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

/// Write the eval task `id` under `suite`: `prompt`, then `verify` with
/// `note.txt` holding "hi" in its workspace.
fn eval_task(suite: &Path, id: &str, prompt: &str, verify: &str) {
    let dir = suite.join(id);
    std::fs::create_dir_all(dir.join(crate::evals::FIXTURE_DIR)).unwrap();
    std::fs::write(dir.join(crate::evals::FIXTURE_DIR).join("note.txt"), "hi\n").unwrap();
    let toml = format!("prompt = {prompt:?}\nverify = {verify:?}\n");
    std::fs::write(dir.join(crate::evals::TASK_FILE), toml).unwrap();
}

#[tokio::test]
async fn an_eval_task_under_gate_is_routed_and_its_verdict_feeds_the_table() {
    use super::EvalTurns;
    use crate::evals::{self, EvalGate, EvalOptions, EvalRunner};
    use crate::event_loop::subagents::lock;
    use aura_agent_gate::Candidate;
    use aura_agent_trace::{RunBundle, RunTrace};
    use aura_core_types::{
        OutcomeSignal, PromptCategory, Route, RouteLevel, RouteReason, ToolProposal,
        TransactionType,
    };
    use aura_model_reasoner::model_catalog::{ModelBackend, ModelTier};
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
    let mode = super::permission_mode(aura_core_modes::AgentMode::Agent);
    let (h, requests) = harness(
        &dirs,
        MockProvider::new().with_default_response(MockResponse::text("question")),
        ToolFilter::default(),
        mode,
        gate,
    );
    let hub = Arc::clone(&h.hub);
    let suite = TempDir::new().unwrap();
    eval_task(
        suite.path(),
        "fails",
        "is there a missing.txt?",
        "test -f missing.txt",
    );
    eval_task(
        suite.path(),
        "passes",
        "what does note.txt say?",
        "[ \"$(cat note.txt)\" = 'hi' ]",
    );
    let tasks = evals::load_suite(suite.path()).unwrap();
    let home = TempDir::new().unwrap();
    let opts = EvalOptions {
        home: home.path().to_path_buf(),
        suite: "s".to_string(),
        arm: Some("gate".to_string()),
        repeat: 1,
        level: aura_config::trace::TraceLevel::Full,
        runner: EvalRunner::Gate(EvalGate::on(EvalTurns::new(h))),
        work_root: home.path().join("work"),
        budget_usd: None,
    };

    let results = tokio::time::timeout(Duration::from_secs(60), evals::run_suite(&tasks, &opts))
        .await
        .expect("the eval did not finish")
        .unwrap();

    let passed: Vec<(&str, bool)> = results
        .iter()
        .map(|r| (r.task.as_str(), r.passed))
        .collect();
    assert_eq!(passed, [("fails", false), ("passes", true)]);
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
    for result in &results {
        let gate = result.gate.as_ref().expect("a gate run says what Gate did");
        assert_eq!(gate.routed, [SONNET]);
        let calls: u32 = gate.tokens.iter().map(|m| m.calls).sum();
        assert_eq!(calls, 2, "the classifier call and the turn");
        assert!(result.error.is_none(), "{:?}", result.error);
    }
    let table = evals::results_table(&results);
    assert!(
        table.contains(&format!("gate routed to {SONNET} (2 model calls")),
        "{table}"
    );

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

    let trace = RunTrace::load(&RunBundle::find(home.path(), &results[1].run_id).unwrap()).unwrap();
    assert_eq!(trace.meta.model, "gate");
    assert_eq!(trace.meta.task_id.as_deref(), Some("s/passes"));
    let card = trace.scorecard();
    assert_eq!(card.outcome, Some(1.0));
    assert_eq!(card.model_calls, 1, "the routed turn is in the run's trace");
    let rewards = trace.rewards();
    let verify = rewards.iter().find(|r| r.name == "verify").unwrap();
    assert_eq!(verify.evidence["via"], "run_command");
}
