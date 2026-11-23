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

/// A mock model that also keeps every request it was sent. Calls are
/// billed as the model asked for, so calls to a catalog model have a
/// list price.
struct Recording {
    inner: MockProvider,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

/// A reply [`Recording`] turns into a failed call, as from a model that
/// fell over.
const FAILS: &str = "<the call fails>";

#[async_trait]
impl ModelProvider for Recording {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        self.requests.lock().unwrap().push(request.clone());
        let model = request.model.as_str().to_string();
        let mut response = self.inner.complete(request).await?;
        if response.message.text_content() == FAILS {
            return Err(ReasonerError::Api {
                status: 400,
                message: "the model fell over".to_string(),
            });
        }
        response.trace.model.clone_from(&model);
        response.model_used = model;
        Ok(response)
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
        gate: aura_config::GateConfig::defaults().without_exploring(),
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
        effort: None,
        held_effort: None,
        mode: permissions.mode.agent_mode(),
        workspace_root: workspace,
        tool_filter: filter,
        job_wait: Duration::from_secs(5),
        trace: None,
        notes: Vec::new(),
        judged: Vec::new(),
        criteria: None,
        criteria_due: false,
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

/// User prompts the model was sent in the last request of an agent turn,
/// not one of Gate's own calls.
fn last_prompts(requests: &Mutex<Vec<ModelRequest>>) -> Vec<String> {
    let requests = requests.lock().unwrap();
    requests
        .iter()
        .rev()
        .find(|request| request.metadata.purpose.is_none())
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
    use aura_core_types::{OutcomeSignal, PromptCategory, Route, RouteReason};
    use aura_gate::route::Candidate;
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
        MockProvider::simple_response("question medium"),
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
    assert_eq!(
        models,
        [HAIKU, SONNET, SONNET],
        "classified, routed, then judged, as the turn ran no check"
    );
    {
        let requests = requests.lock().unwrap();
        let turn = &requests[1];
        let tools: Vec<&str> = turn.tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            tools.contains(&"write_file") && tools.contains(&"edit_file"),
            "a question is answered by the model it is routed to: {tools:?}"
        );
        assert!(!turn.system.contains("<gate_lead>"));
        assert_eq!(
            requests[2].system,
            aura_gate::judge::turn::UNVERIFIED_SYSTEM_PROMPT
        );
    }
    let price = |model: &str| {
        aura_model::model_catalog::estimate_cost_usd(model, &aura_model::Usage::new(100, 50))
            .unwrap()
    };
    let about =
        |cost: Option<f64>, expected: f64| cost.is_some_and(|c| (c - expected).abs() < 1e-9);
    assert!(
        about(report.usage.cost_usd, price(SONNET)),
        "the agent's call: {:?}",
        report.usage
    );
    assert!(
        about(report.gate_cost_usd, price(HAIKU) + price(SONNET)),
        "the classifier's call and the judge's: {:?}",
        report.gate_cost_usd
    );
    assert!(
        about(report.total_cost_usd, price(HAIKU) + 2.0 * price(SONNET)),
        "the run's cost counts Gate's calls: {:?}",
        report.total_cost_usd
    );
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
    assert_eq!(prompts[0].category, Some(PromptCategory::Question));
    assert_eq!(prompts[0].classifier, HAIKU);
    assert_eq!(
        prompts[0].route,
        Route::turn(SONNET, RouteReason::Default).at_effort("medium")
    );
    let outcomes: Vec<_> = entries
        .iter()
        .filter_map(|e| e.tx.outcome_payload())
        .collect();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].model, SONNET);
    assert_eq!(outcomes[0].signal, OutcomeSignal::Unavailable);
    assert_eq!(h.hub.gate.recent().len(), 1);
}

#[tokio::test]
async fn a_headless_turn_changing_several_files_is_led() {
    use aura_gate::route::Candidate;
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
        MockProvider::simple_response("multi_file_change medium"),
        ToolFilter::default(),
        permissions,
        gate,
    );
    for (led, outcomes) in [(true, (10, true)), (false, (3, false))] {
        h.hub.gate.learn(
            aura_core_types::RouteLevel::Turn,
            (
                aura_core_types::PromptCategory::MultiFileChange,
                Some(aura_core_types::Difficulty::Medium),
            ),
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
    assert_eq!(
        requests[1].system,
        aura_gate::judge::criteria::SYSTEM_PROMPT,
        "what the change is checked against is asked for before it starts"
    );
    let lead = &requests[2];
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
    use aura_agent_trace::{RunBundle, RunMeta, RunRecorder, RunSource, RunTrace};
    use aura_core_types::{
        OutcomeSignal, PromptCategory, Route, RouteLevel, RouteReason, ToolProposal,
        TransactionType,
    };
    use aura_gate::route::Candidate;
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
        MockProvider::new().with_default_response(MockResponse::text("question medium")),
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
        [HAIKU, SONNET, SONNET, HAIKU, SONNET, SONNET],
        "each task classified, routed, then judged, as its turn ran no check"
    );
    for (ran, _, _) in &runs {
        assert_eq!(ran.gate.routed, [SONNET]);
        let calls: u32 = ran.gate.tokens.iter().map(|m| m.calls).sum();
        assert_eq!(calls, 3, "the classifier call, the turn and the judge's");
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
        assert_eq!(prompts[0].category, Some(PromptCategory::Question));
        assert_eq!(
            prompts[0].route,
            Route::turn(SONNET, RouteReason::Default).at_effort("medium")
        );
        let signals: Vec<OutcomeSignal> = entries
            .iter()
            .filter_map(|e| e.tx.outcome_payload())
            .map(|o| o.signal)
            .collect();
        assert_eq!(
            signals,
            [OutcomeSignal::Unavailable, verdict],
            "{}",
            agent.name
        );
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
    routes: Vec<(
        Option<aura_core_types::PromptCategory>,
        aura_core_types::Route,
    )>,
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
    use aura_gate::route::Candidate;
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

/// The criteria writer's answer when it writes none down. Gate asks for
/// them right after classifying the prompt of code work, and the work
/// goes on without them.
fn no_criteria() -> MockResponse {
    MockResponse::text("No criteria.")
}

/// A model whose prompt is classified as debugging, with no criteria to
/// check it against, and whose turn writes `parser.txt`, then runs
/// `cargo test` (which fails in a workspace with no crate) and gives up.
/// The model calls after that are answered with `then`, in order.
fn failing_check_provider(then: &[&str]) -> MockProvider {
    let mut provider = MockProvider::new()
        .with_response(MockResponse::text("debugging medium"))
        .with_response(no_criteria())
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
            GATE_SONNET,
            GATE_OPUS,
            GATE_OPUS,
            GATE_OPUS
        ],
        "classified, its criteria asked for, worked on Sonnet, judged by Opus, moved up a \
         tier, and the unchecked work there judged"
    );
    {
        let requests = run.requests.lock().unwrap();
        let judge = &requests[5];
        assert_eq!(judge.system.as_str(), aura_gate::judge::turn::SYSTEM_PROMPT);
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
                Some(PromptCategory::Debugging),
                Route::turn(GATE_SONNET, RouteReason::Default).at_effort("medium")
            ),
            (
                Some(PromptCategory::Debugging),
                Route::turn(GATE_OPUS, RouteReason::Escalate).at_effort("medium")
            ),
        ]
    );
    assert_eq!(
        run.outcomes,
        [
            (GATE_SONNET.to_string(), OutcomeSignal::ChecksFailed),
            (GATE_SONNET.to_string(), OutcomeSignal::JudgedFail),
            (GATE_OPUS.to_string(), OutcomeSignal::Unavailable),
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
        3,
        "the judge's call, the turn it sent on, and the judge's on that turn's \
         unchecked work: {asked:?}"
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
            (GATE_OPUS.to_string(), OutcomeSignal::Unavailable),
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
        [
            GATE_HAIKU,
            GATE_SONNET,
            GATE_SONNET,
            GATE_SONNET,
            GATE_SONNET,
            GATE_OPUS
        ],
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
            Some(PromptCategory::Debugging),
            Route::turn(GATE_SONNET, RouteReason::Default).at_effort("medium")
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
            (GATE_OPUS.to_string(), OutcomeSignal::Unavailable),
        ],
        "the turn's own signal stands"
    );
    assert_eq!(run.rows, [Some((1, 0)), None]);
}

#[tokio::test]
async fn gates_own_check_streams_what_it_found() {
    use crate::event_loop::gate::followup::{Found, Report};
    use crate::event_loop::task_check::CheckRun;
    let captured = Captured::default();
    let output = Output::new(OutputFormat::StreamJson, Box::new(captured.clone()));
    let check = CheckRun {
        command: "cargo test".to_string(),
        passed: Some(false),
        exit_code: Some(101),
        tail: "test parse ... FAILED".to_string(),
    };

    output.report(Found::Checking("cargo test")).await;
    let checked = Found::Checked {
        model: GATE_SONNET,
        command: "cargo test",
        check: &check,
        signal: check.signal(),
    };
    output.report(checked).await;

    assert_eq!(
        captured.json_lines(),
        [json!({
            "type": "checked",
            "model": GATE_SONNET,
            "command": "cargo test",
            "passed": false,
            "summary": check.describe(),
        })],
        "the check is streamed once it has run, not as it starts"
    );
}

/// Run "fix the parser" under [`sonnet_and_opus`] on `provider`, which
/// answers the classifier first. Returns the report, the model of each
/// call, the requests, each `Outcome`, and the attempts in Gate's
/// attempt log.
async fn run_routed(
    provider: MockProvider,
) -> (
    super::RunReport,
    Vec<String>,
    Arc<Mutex<Vec<ModelRequest>>>,
    Vec<aura_core_types::OutcomePayload>,
    Vec<aura_gate::attempts::Attempt>,
) {
    use aura_gate::attempts::{AttemptLog, Line};
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
    let attempts = AttemptLog::in_data_dir(dirs.data.path())
        .read()
        .unwrap()
        .lines
        .into_iter()
        .filter_map(|line| match line {
            Line::Attempt(attempt) => Some(attempt),
            _ => None,
        })
        .collect();
    (report, models, requests, outcomes, attempts)
}

#[tokio::test]
async fn a_change_made_without_a_check_is_checked_before_the_turn_ends() {
    use aura_core_types::{Difficulty, OutcomeSignal};
    let provider = MockProvider::new()
        .with_response(MockResponse::text("debugging medium"))
        .with_response(no_criteria())
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
    let (report, models, requests, outcomes, attempts) = run_routed(provider).await;

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
            GATE_SONNET,
            GATE_OPUS,
            GATE_OPUS,
            GATE_OPUS
        ],
        "Sonnet is asked to check its change, its check fails, the judge blames it, Opus \
         takes over and its unchecked work is judged"
    );
    let nudge = requests.lock().unwrap()[4]
        .messages
        .last()
        .unwrap()
        .text_content();
    assert!(nudge.contains(aura_gate::followup::NUDGE_PROMPT), "{nudge}");
    let signals: Vec<(&str, OutcomeSignal)> = outcomes
        .iter()
        .map(|o| (o.model.as_str(), o.signal))
        .collect();
    assert_eq!(
        signals,
        [
            (GATE_SONNET, OutcomeSignal::ChecksFailed),
            (GATE_SONNET, OutcomeSignal::JudgedFail),
            (GATE_OPUS, OutcomeSignal::Unavailable),
        ],
        "the unchecked turn's outcome waits for the check it is asked for"
    );
    let sonnet: Vec<_> = attempts.iter().filter(|a| a.model == GATE_SONNET).collect();
    assert_eq!(
        sonnet.len(),
        1,
        "the turn and the check it was asked for are logged once: {attempts:?}"
    );
    assert_eq!(
        (sonnet[0].signal, sonnet[0].prompt_tx.as_str()),
        (
            OutcomeSignal::ChecksFailed,
            outcomes[0].prompt_tx.to_hex().as_str()
        ),
        "the attempt's line says what its record says"
    );
    let price =
        aura_model::model_catalog::estimate_cost_usd(GATE_SONNET, &aura_model::Usage::new(100, 50))
            .unwrap();
    assert!(
        (outcomes[0].cost_usd - 4.0 * price).abs() < 1e-9,
        "the check speaks for the turn that made the change, whose two calls it costs \
         along with its own two: {}",
        outcomes[0].cost_usd
    );
    let ran: Vec<(&str, Option<Difficulty>)> = attempts
        .iter()
        .map(|a| (a.model.as_str(), a.difficulty))
        .collect();
    assert!(
        ran.iter().any(|(model, _)| *model == GATE_OPUS)
            && ran
                .iter()
                .all(|(_, difficulty)| *difficulty == Some(Difficulty::Medium)),
        "the check it was asked for and the stronger model keep the work's difficulty: {ran:?}"
    );
}

#[tokio::test]
async fn a_change_the_model_ran_itself_is_not_nudged_but_still_judged() {
    use aura_core_types::OutcomeSignal;
    let provider = MockProvider::new()
        .with_response(MockResponse::text("debugging medium"))
        .with_response(no_criteria())
        .with_response(MockResponse::tool_use(
            "c0",
            "write_file",
            json!({"path": "parser.sh", "content": "printf '%s\\n' \"$@\"\n"}),
        ))
        .with_response(MockResponse::tool_use(
            "c1",
            "run_command",
            json!({"program": "sh", "args": ["-n", "parser.sh"]}),
        ))
        .with_response(MockResponse::text("Fixed the parser; `sh -n` passes."))
        .with_response(MockResponse::text("The change is complete.\nVERDICT: OK"));
    let (report, models, requests, outcomes, _) = run_routed(provider).await;

    assert_eq!(
        (report.model.as_str(), report.result.as_str()),
        (GATE_SONNET, "Fixed the parser; `sh -n` passes.")
    );
    assert_eq!(
        models,
        [
            GATE_HAIKU,
            GATE_SONNET,
            GATE_SONNET,
            GATE_SONNET,
            GATE_SONNET,
            GATE_SONNET
        ],
        "the work's three calls, then the judge, with no nudge between"
    );
    {
        let requests = requests.lock().unwrap();
        assert!(requests
            .iter()
            .flat_map(|request| &request.messages)
            .all(|message| !aura_gate::evidence::transcript::said::is_nudge(
                &message.text_content()
            )));
        assert_eq!(
            requests[5].system.as_str(),
            aura_gate::judge::turn::UNVERIFIED_SYSTEM_PROMPT
        );
    }
    let signals: Vec<(&str, OutcomeSignal)> = outcomes
        .iter()
        .map(|o| (o.model.as_str(), o.signal))
        .collect();
    assert_eq!(signals, [(GATE_SONNET, OutcomeSignal::JudgedPass)]);
}

#[tokio::test]
async fn unchecked_work_that_looks_unfinished_goes_to_the_judge_then_moves_up() {
    use aura_core_types::OutcomeSignal;
    let provider = MockProvider::new()
        .with_response(MockResponse::text("debugging medium"))
        .with_response(no_criteria())
        .with_response(MockResponse::text(
            "I could not find where the parser drops the token.",
        ))
        .with_response(MockResponse::text(
            "It never opened the parser.\nVERDICT: MODEL",
        ))
        .with_response(MockResponse::text("Fixed it."));
    let (report, models, requests, outcomes, _) = run_routed(provider).await;

    assert_eq!(
        (report.model.as_str(), report.result.as_str()),
        (GATE_OPUS, "Fixed it.")
    );
    assert_eq!(
        models,
        [
            GATE_HAIKU,
            GATE_SONNET,
            GATE_SONNET,
            GATE_SONNET,
            GATE_OPUS,
            GATE_OPUS
        ],
        "Sonnet's unchecked work is judged by Sonnet, Opus's by Opus"
    );
    {
        let requests = requests.lock().unwrap();
        let judge = &requests[3];
        assert_eq!(
            judge.system.as_str(),
            aura_gate::judge::turn::UNVERIFIED_SYSTEM_PROMPT
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
            (GATE_SONNET, OutcomeSignal::JudgedFail),
            (GATE_OPUS, OutcomeSignal::Unavailable),
        ],
        "the judge's verdict is the unchecked work's outcome"
    );
}

#[tokio::test]
async fn a_turn_whose_follow_up_changes_its_signal_is_recorded_once() {
    use aura_core_types::OutcomeSignal;
    let provider = MockProvider::new()
        .with_response(MockResponse::text("debugging easy"))
        .with_response(no_criteria())
        .with_response(MockResponse::tool_use(
            "c0",
            "run_command",
            json!({"program": "cargo", "args": ["check", "--help"]}),
        ))
        .with_response(MockResponse::text(
            "I made the change, but it isn't verified.",
        ))
        .with_response(MockResponse::text(
            "The change is untested and wrong.\nVERDICT: MODEL",
        ))
        .with_response(MockResponse::text("Fixed it."));
    let (report, _, _, outcomes, attempts) = run_routed(provider).await;

    assert_eq!(report.model, GATE_OPUS, "{:?}", report.error);
    let signals: Vec<(&str, OutcomeSignal)> = outcomes
        .iter()
        .map(|o| (o.model.as_str(), o.signal))
        .collect();
    assert_eq!(
        signals,
        [
            (GATE_SONNET, OutcomeSignal::JudgedFail),
            (GATE_OPUS, OutcomeSignal::Unavailable),
        ],
        "the check that passed is not recorded once the judge finds the work unfinished"
    );
    let logged: Vec<(&str, OutcomeSignal)> = attempts
        .iter()
        .map(|a| (a.model.as_str(), a.signal))
        .collect();
    assert_eq!(logged, signals, "each attempt is logged once, as recorded");
}

/// The `judged` and `notice` lines a run of "fix the parser" under
/// [`sonnet_and_opus`] on `provider` streamed, the prompts it recorded,
/// and the user messages of its last agent turn.
async fn moved_up(
    provider: MockProvider,
) -> (Vec<Value>, Vec<aura_core_types::PromptPayload>, Vec<String>) {
    let dirs = dirs();
    let permissions = super::permissions(aura_core_modes::AgentMode::Agent, None);
    let gate = sonnet_and_opus();
    let (mut h, requests) = harness(&dirs, provider, ToolFilter::default(), permissions, gate);
    let (report, captured) = execute(
        &mut h,
        request("fix the parser", SessionChoice::New),
        OutputFormat::StreamJson,
    )
    .await;
    let lines = (captured.json_lines().into_iter())
        .filter(|line| line["type"] == "judged" || line["type"] == "notice")
        .collect();
    let (agent, _) = parse_session_id(&report.session_id, AgentId::generate()).unwrap();
    let entries = h.hub.factory.store.scan_record(agent, 1, 1_000).unwrap();
    let prompts = entries
        .iter()
        .filter_map(|e| e.tx.prompt_payload())
        .collect();
    (lines, prompts, last_prompts(&requests))
}

/// A run [`moved_up`] goes through: its provider, the `judged` line it
/// streams, what Opus is told when it takes the work over (`None` when
/// it does not), and the user messages of the turn before.
type MoveUp = (MockProvider, Value, Option<&'static str>, Vec<&'static str>);

/// A failed check the judge blames, one it excuses, and unchecked work
/// it finds unfinished.
fn move_ups() -> [MoveUp; 3] {
    let failed = |verdict| failing_check_provider(&[verdict, "Fixed it."]);
    let unchecked = MockProvider::new()
        .with_response(MockResponse::text("debugging medium"))
        .with_response(no_criteria())
        .with_response(MockResponse::text(
            "I could not find where the parser drops the token.",
        ))
        .with_response(MockResponse::text(
            "It never opened the parser.\nVERDICT: MODEL",
        ))
        .with_response(MockResponse::text("Fixed it."));
    let judged = |judge: &str, at_fault: bool, cost_usd: f64, reasons: &str| {
        json!({"type": "judged", "model": GATE_SONNET, "judge": judge, "at_fault": at_fault,
               "reasons": reasons, "cost_usd": cost_usd})
    };
    let note = "NOTE: You've made your first file change. Before making more changes, consider \
                verifying your work (e.g., run the build or tests) to catch issues early.";
    [
        (
            failed("The parser still drops the last token.\nVERDICT: MODEL"),
            judged(
                GATE_OPUS,
                true,
                0.00175,
                "The parser still drops the last token.",
            ),
            Some(concat!(
             "The work above was done on aura-claude-sonnet-4-6 (cloud) and ended with its last ",
             "test or build command failing. A reviewer of that work found: The parser still ",
             "drops the last token. You are a stronger model taking over this turn. The files ",
             "are as that work left them: check what it changed, find what is wrong and finish ",
             "the request."
         )),
            vec!["fix the parser", note],
        ),
        (
            failed("There is no crate here to test.\nVERDICT: OTHER"),
            judged(GATE_OPUS, false, 0.00175, "There is no crate here to test."),
            None,
            vec!["fix the parser", note],
        ),
        (
            unchecked,
            judged(GATE_SONNET, true, 0.00105, "It never opened the parser."),
            Some(concat!(
                "The work above was done on aura-claude-sonnet-4-6 (cloud) and ended without ",
                "checking its work, which looked unfinished. A reviewer of that work found: It ",
                "never opened the parser. You are a stronger model taking over this turn. The ",
                "files are as that work left them: check what it changed, find what is wrong and ",
                "finish the request."
            )),
            vec!["fix the parser"],
        ),
    ]
}

#[tokio::test]
async fn a_move_up_streams_records_and_sends_its_takeover_byte_for_byte() {
    use aura_core_types::{Difficulty, PromptCategory, PromptPayload, Route, RouteReason};
    let asked = PromptPayload {
        category: Some(PromptCategory::Debugging),
        classifier: GATE_HAIKU.to_string(),
        route: Route::turn(GATE_SONNET, RouteReason::Default).at_effort("medium"),
        led: Some(false),
        difficulty: Some(Difficulty::Medium),
        ..PromptPayload::new("fix the parser")
    };
    for (provider, judged, takeover, mut sent) in move_ups() {
        let (lines, prompts, last) = moved_up(provider).await;
        let mut streamed = vec![judged];
        streamed.extend(takeover.map(|text| json!({"type": "notice", "text": text})));
        assert_eq!(lines, streamed);
        let told = takeover.map(|text| format!("Harness notice:\n{text}"));
        let mut recorded = vec![asked.clone()];
        recorded.extend(told.clone().map(|text| PromptPayload {
            category: Some(PromptCategory::Debugging),
            route: Route::turn(GATE_OPUS, RouteReason::Escalate).at_effort("medium"),
            led: Some(false),
            ..PromptPayload::new(text)
        }));
        assert_eq!(prompts, recorded);
        sent.extend(told.as_deref());
        assert_eq!(last, sent);
    }
}

/// What a run of "fix the parser" under [`sonnet_and_opus`] left: its
/// report, the lines it printed, the requests it made, its record and
/// the attempt lines of Gate's attempt log, each line as written.
struct Routed {
    report: super::RunReport,
    printed: Vec<String>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    entries: Vec<aura_core_types::RecordEntry>,
    attempts: Vec<String>,
}

/// An outcome as (index of its prompt, category, model, success, signal,
/// cost in USD).
type Outcome = (
    usize,
    aura_core_types::PromptCategory,
    String,
    Option<bool>,
    aura_core_types::OutcomeSignal,
    f64,
);

/// Run "fix the parser" under [`sonnet_and_opus`] on `provider`, in a
/// workspace `prepare` fills first.
async fn routed(provider: MockProvider, prepare: impl FnOnce(&Path)) -> Routed {
    let dirs = dirs();
    prepare(dirs.workspace.path());
    let permissions = super::permissions(aura_core_modes::AgentMode::Agent, None);
    let gate = sonnet_and_opus();
    let (mut h, requests) = harness(&dirs, provider, ToolFilter::default(), permissions, gate);
    let asked = request("fix the parser", SessionChoice::New);
    let (report, captured) = execute(&mut h, asked, OutputFormat::StreamJson).await;
    let (agent, _) = parse_session_id(&report.session_id, AgentId::generate()).unwrap();
    let entries = h.hub.factory.store.scan_record(agent, 1, 1_000).unwrap();
    let log = aura_gate::attempts::AttemptLog::in_data_dir(dirs.data.path());
    let logged = std::fs::read_to_string(log.path()).unwrap_or_default();
    let attempts = (logged.lines())
        .filter(|line| line.starts_with(r#"{"kind":"attempt","#))
        .map(str::to_string)
        .collect();
    Routed {
        report,
        printed: captured.text().lines().map(str::to_string).collect(),
        requests,
        entries,
        attempts,
    }
}

impl Routed {
    /// The lines it streamed of what Gate did: its notices, its checks
    /// and its judges' findings.
    fn gate_lines(&self) -> Vec<&str> {
        let gate = ["notice", "checked", "judged"].map(|kind| format!(r#"{{"type":"{kind}","#));
        (self.printed.iter().map(String::as_str))
            .filter(|line| gate.iter().any(|start| line.starts_with(start.as_str())))
            .collect()
    }

    /// The prompts it recorded.
    fn prompts(&self) -> Vec<aura_core_types::PromptPayload> {
        (self.entries.iter())
            .filter_map(|e| e.tx.prompt_payload())
            .collect()
    }

    /// The outcomes it recorded, each naming its prompt by index.
    fn outcomes(&self) -> Vec<Outcome> {
        let prompts: Vec<_> = (self.entries.iter())
            .filter(|e| e.tx.prompt_payload().is_some())
            .map(|e| e.tx.hash)
            .collect();
        (self.entries.iter())
            .filter_map(|e| e.tx.outcome_payload())
            .map(|o| {
                let at = prompts.iter().position(|hash| *hash == o.prompt_tx);
                (
                    at.unwrap(),
                    o.category,
                    o.model,
                    o.success,
                    o.signal,
                    o.cost_usd,
                )
            })
            .collect()
    }
}

/// The prompt of "fix the parser" as Gate records it when the turn starts
/// on Sonnet.
fn asked_on_sonnet() -> aura_core_types::PromptPayload {
    use aura_core_types::{Difficulty, PromptCategory, PromptPayload, Route, RouteReason};
    PromptPayload {
        category: Some(PromptCategory::Debugging),
        classifier: GATE_HAIKU.to_string(),
        route: Route::turn(GATE_SONNET, RouteReason::Default).at_effort("medium"),
        led: Some(false),
        difficulty: Some(Difficulty::Medium),
        ..PromptPayload::new("fix the parser")
    }
}

/// A model whose prompt is classified as debugging, with no criteria to
/// check it against, and whose turn writes `parser.txt` and says it is
/// done without checking; it then answers `then`, in order.
fn unchecked_change_provider(then: &[&str]) -> MockProvider {
    let mut provider = MockProvider::new()
        .with_response(MockResponse::text("debugging medium"))
        .with_response(no_criteria())
        .with_response(MockResponse::tool_use(
            "c0",
            "write_file",
            json!({"path": "parser.txt", "content": "skip the last token\n"}),
        ))
        .with_response(MockResponse::text("Fixed the parser."));
    for reply in then {
        provider = provider.with_response(MockResponse::text(*reply));
    }
    provider
}

/// What the agent loop tells the model once it has changed its first
/// file.
const FIRST_CHANGE_NOTE: &str = "NOTE: You've made your first file change. Before making more \
                                 changes, consider verifying your work (e.g., run the build or \
                                 tests) to catch issues early.";

/// A run whose change Sonnet is asked to check, which it does not, and
/// whose unchecked work the judge then lets stand.
async fn nudged() -> Routed {
    let then = ["I read it again.", "It is done.\nVERDICT: OK"];
    routed(unchecked_change_provider(&then), |_| {}).await
}

#[tokio::test]
async fn a_nudge_streams_records_and_sends_its_message_byte_for_byte() {
    use aura_core_types::{OutcomeSignal, PromptCategory, PromptPayload};
    let run = nudged().await;

    assert_eq!(
        run.gate_lines(),
        [
            "{\"type\":\"notice\",\"text\":\"gate: aura-claude-sonnet-4-6 (cloud) changed files \
             without running a test or build; asking it to check its change\"}",
            "{\"type\":\"judged\",\"model\":\"aura-claude-sonnet-4-6\",\"judge\":\
             \"aura-claude-sonnet-4-6\",\"at_fault\":false,\"reasons\":\"It is done.\",\
             \"cost_usd\":0.00105}",
        ]
    );
    let nudge = format!("Harness notice:\n{}", aura_gate::followup::NUDGE_PROMPT);
    let sent = ["fix the parser", FIRST_CHANGE_NOTE, &nudge];
    assert_eq!(last_prompts(&run.requests), sent);
    let nudged = PromptPayload {
        led: Some(false),
        ..PromptPayload::new(nudge)
    };
    assert_eq!(run.prompts(), [asked_on_sonnet(), nudged]);
    let (debugging, passed) = (PromptCategory::Debugging, OutcomeSignal::JudgedPass);
    let outcome = (
        0,
        debugging,
        GATE_SONNET.to_string(),
        Some(true),
        passed,
        0.00315,
    );
    assert_eq!(run.outcomes(), [outcome]);
}

#[tokio::test]
async fn a_nudged_attempt_is_logged_once_byte_for_byte() {
    let run = nudged().await;

    let [line] = run.attempts.as_slice() else {
        panic!("one attempt line: {:?}", run.attempts);
    };
    let attempt: Value = serde_json::from_str(line).unwrap();
    let (agent, _) = parse_session_id(&run.report.session_id, AgentId::generate()).unwrap();
    let asked = run.entries.iter().find(|e| e.tx.prompt_payload().is_some());
    let logged = format!(
        "{{\"kind\":\"attempt\",\"at_ms\":{},\"agent\":\"{}\",\"prompt_tx\":\"{}\",\
         \"category\":\"debugging\",\"difficulty\":\"medium\",\"led\":false,\"routed\":true,\
         \"model\":\"aura-claude-sonnet-4-6\",\"signal\":\"judged_pass\",\"success\":true,\
         \"cost_usd\":0.00315,\"subtask_cost_usd\":0.0,\"latency_ms\":{},\"evidence\":\
         {{\"calls\":3,\"commands\":0,\"checks_run\":0,\"nudges\":1,\"files_edited\":\
         [\"parser.txt\"],\"files_written\":[\"parser.txt\"],\"files_changed\":1,\
         \"hit_limit\":false,\"reply\":\"I read it again.\"}}}}",
        attempt["at_ms"],
        agent.to_hex(),
        asked.unwrap().tx.hash.to_hex(),
        attempt["latency_ms"],
    );
    assert_eq!(*line, logged);
}

#[tokio::test]
async fn criteria_are_sent_after_the_prompt_but_not_recorded_with_it() {
    let criteria = json!({"items": [{"text": "the parser keeps the last token",
                                     "kind": "behaviour"}], "strict": false});
    let provider = MockProvider::new()
        .with_response(MockResponse::text("debugging medium"))
        .with_response(MockResponse::tool_use("k0", "criteria", criteria))
        .with_response(MockResponse::text("I could not find the parser."))
        .with_response(MockResponse::text("It never looked.\nVERDICT: OK"));
    let run = routed(provider, |_| {}).await;

    let sent = "fix the parser\n\nBefore you started, the harness wrote down what this work \
                will be checked against. Before you finish, make sure each holds and that a \
                test or command you ran shows it. Where one names a reference, compare against \
                that reference itself on every path the change adds, rather than against \
                output written out by hand.\n\n1. [behaviour] the parser keeps the last token";
    assert_eq!(last_prompts(&run.requests), [sent]);
    assert_eq!(run.prompts(), [asked_on_sonnet()]);
    assert_eq!(
        run.gate_lines(),
        [
            "{\"type\":\"judged\",\"model\":\"aura-claude-sonnet-4-6\",\"judge\":\
          \"aura-claude-sonnet-4-6\",\"at_fault\":false,\"reasons\":\"It never looked.\",\
          \"cost_usd\":0.00105}"
        ]
    );
}

/// A model whose prompt is classified as debugging, with no criteria to
/// check it against, and whose turn writes `parser.txt`, then runs its
/// tests through the workspace's `.swe/run`, which fails, and gives up;
/// the judge then excuses the failure.
fn failing_script_provider() -> MockProvider {
    MockProvider::new()
        .with_response(MockResponse::text("debugging medium"))
        .with_response(no_criteria())
        .with_response(MockResponse::tool_use(
            "c0",
            "write_file",
            json!({"path": "parser.txt", "content": "skip the last token\n"}),
        ))
        .with_response(MockResponse::tool_use(
            "c1",
            "run_command",
            json!({"program": "sh", "args": [".swe/run", "pytest"]}),
        ))
        .with_response(MockResponse::text("The tests still fail."))
        .with_response(MockResponse::text("No crate here.\nVERDICT: OTHER"))
}

/// A `.swe/run` in `workspace` that prints a failing test and fails.
fn failing_script(workspace: &Path) {
    std::fs::create_dir_all(workspace.join(".swe")).unwrap();
    let script = "echo 'FAILED test_parser.py::test_last_token'\necho '1 failed'\nexit 1\n";
    std::fs::write(workspace.join(".swe/run"), script).unwrap();
}

#[tokio::test]
async fn the_judge_of_a_failed_turn_is_shown_it_byte_for_byte() {
    let run = routed(failing_script_provider(), failing_script).await;

    let requests = run.requests.lock().unwrap();
    let judge = &requests[5];
    assert_eq!(judge.model.as_str(), GATE_OPUS);
    assert_eq!(judge.system.as_str(), aura_gate::judge::turn::SYSTEM_PROMPT);
    assert_eq!(judge.messages.len(), 1);
    assert_eq!(
        judge.messages[0].text_content(),
        "## The user's request\n\nfix the parser\n\n## Changes\n```diff\n=== parser.txt\n\
         @@ -0,0 +1 @@\n+skip the last token\n\n```\n\n## Final reply\nThe tests still fail.\
         \n\n## How it failed\nThe last test or build command it ran, `sh .swe/run pytest`, \
         failed. What the command printed:\n{\"exit_code\":1,\"ok\":false,\"stderr\":\
         \"exit_code: 1\\nstdout:\\nFAILED test_parser.py::test_last_token\\n1 failed\\n\\n\
         stderr:\\n\",\"stdout\":\"\"}"
    );
}

#[tokio::test]
async fn a_move_up_prints_its_report_byte_for_byte() {
    let then = [
        "The parser still drops the last token.\nVERDICT: MODEL",
        "Fixed it.",
    ];
    let run = routed(failing_check_provider(&then), |_| {}).await;

    let report = &run.report;
    let printed = format!(
        "{{\"type\":\"result\",\"exit_reason\":\"success\",\"is_error\":false,\"result\":\
         \"Fixed it.\",\"session_id\":\"{}\",\"model\":\"aura-claude-opus-4-7\",\
         \"num_turns\":4,\"agent_turns\":2,\"duration_ms\":{},\"usage\":{{\"calls\":4,\
         \"input_tokens\":400,\"cache_read_tokens\":0,\"cache_write_tokens\":0,\
         \"output_tokens\":200,\"cache_hit_rate\":0.0,\"cost_usd\":0.0049}},\
         \"total_cost_usd\":0.0098,\"gate_cost_usd\":0.0049,\"compactions\":0,\"todos\":[],\
         \"files_changed\":[\"parser.txt\"],\"error\":null}}",
        report.session_id, report.duration_ms,
    );
    assert_eq!(run.printed.last(), Some(&printed));
}

#[tokio::test]
async fn a_model_error_moves_the_turn_up_byte_for_byte() {
    use aura_core_types::{OutcomeSignal, PromptCategory, PromptPayload, Route, RouteReason};
    let provider = MockProvider::new()
        .with_response(MockResponse::text("debugging medium"))
        .with_response(no_criteria())
        .with_response(MockResponse::text(FAILS))
        .with_response(MockResponse::text("It broke.\nVERDICT: MODEL"))
        .with_response(MockResponse::text("Fixed it."));
    let run = routed(provider, |_| {}).await;

    let report = &run.report;
    assert_eq!(
        (report.exit_reason, report.error.as_deref()),
        (ExitReason::Success, None)
    );
    assert_eq!(
        (report.model.as_str(), report.result.as_str()),
        (GATE_OPUS, "Fixed it.")
    );
    let told = "The work above was done on aura-claude-sonnet-4-6 (cloud) and ended in an error. \
                A reviewer of that work found: It broke. You are a stronger model taking over \
                this turn. The files are as that work left them: check what it changed, find \
                what is wrong and finish the request.";
    let judged = "{\"type\":\"judged\",\"model\":\"aura-claude-sonnet-4-6\",\"judge\":\
                  \"aura-claude-opus-4-7\",\"at_fault\":true,\"reasons\":\"It broke.\",\
                  \"cost_usd\":0.00175}";
    let notice = format!("{{\"type\":\"notice\",\"text\":\"{told}\"}}");
    assert_eq!(run.gate_lines(), [judged, &notice]);
    let takeover = PromptPayload {
        category: Some(PromptCategory::Debugging),
        route: Route::turn(GATE_OPUS, RouteReason::Escalate).at_effort("medium"),
        led: Some(false),
        ..PromptPayload::new(format!("Harness notice:\n{told}"))
    };
    assert_eq!(run.prompts(), [asked_on_sonnet(), takeover]);
    let (debugging, sonnet) = (PromptCategory::Debugging, GATE_SONNET.to_string());
    assert_eq!(
        run.outcomes(),
        [
            (
                0,
                debugging,
                sonnet.clone(),
                Some(false),
                OutcomeSignal::Error,
                0.0
            ),
            (
                0,
                debugging,
                sonnet,
                Some(false),
                OutcomeSignal::JudgedFail,
                0.0
            ),
            (
                1,
                debugging,
                GATE_OPUS.to_string(),
                None,
                OutcomeSignal::Unavailable,
                0.00175
            ),
        ]
    );
}

#[tokio::test]
async fn a_led_turn_records_its_prompt_as_led() {
    use aura_core_types::{Difficulty, OutcomeSignal, PromptCategory, PromptPayload};
    use aura_core_types::{Route, RouteLevel, RouteReason};
    let gate = GateBase {
        candidates: Some(vec![sonnet_and_opus().candidates.unwrap()[0].clone()]),
        ..sonnet_and_opus()
    };
    let dirs = dirs();
    let permissions = super::permissions(aura_core_modes::AgentMode::Agent, None);
    let provider = MockProvider::simple_response("multi_file_change medium");
    let (mut h, _) = harness(&dirs, provider, ToolFilter::default(), permissions, gate);
    let work = (PromptCategory::MultiFileChange, Some(Difficulty::Medium));
    for (led, outcomes) in [(true, (10, true)), (false, (3, false))] {
        (h.hub.gate).learn(RouteLevel::Turn, work, GATE_SONNET, Some(led), outcomes);
    }
    let asked = request("rename the crate everywhere", SessionChoice::New);
    let (report, _) = execute(&mut h, asked, OutputFormat::Json).await;

    let (agent, _) = parse_session_id(&report.session_id, AgentId::generate()).unwrap();
    let entries = h.hub.factory.store.scan_record(agent, 1, 1_000).unwrap();
    let prompts: Vec<_> = entries
        .iter()
        .filter_map(|e| e.tx.prompt_payload())
        .collect();
    let led = PromptPayload {
        category: Some(PromptCategory::MultiFileChange),
        classifier: GATE_HAIKU.to_string(),
        route: Route::turn(GATE_SONNET, RouteReason::Default).at_effort("medium"),
        led: Some(true),
        difficulty: Some(Difficulty::Medium),
        ..PromptPayload::new("rename the crate everywhere")
    };
    assert_eq!(prompts, [led]);
    let outcomes: Vec<_> = (entries.iter().filter_map(|e| e.tx.outcome_payload()))
        .map(|o| (o.prompt_tx, o.model, o.signal))
        .collect();
    let asked = entries.iter().find(|e| e.tx.prompt_payload().is_some());
    let unavailable = OutcomeSignal::Unavailable;
    let outcome = (asked.unwrap().tx.hash, GATE_SONNET.to_string(), unavailable);
    assert_eq!(outcomes, [outcome]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_notice_starts_a_routed_turn_byte_for_byte() {
    use aura_core_types::{Difficulty, PromptCategory, PromptPayload, Route, RouteReason};
    let provider = MockProvider::new()
        .with_response(MockResponse::text("question medium"))
        .with_response(MockResponse::tool_use(
            "b0",
            "run_command",
            json!({"program": "sh", "args": ["-c", "sleep 1; echo done"], "background": true}),
        ))
        .with_response(MockResponse::text("Started it."))
        .with_response(MockResponse::text("It is done.\nVERDICT: OK"))
        .with_response(MockResponse::text("question medium"))
        .with_response(MockResponse::text("Noted."));
    let run = routed(provider, |_| {}).await;

    let lines = run.gate_lines();
    let notice: Value = serde_json::from_str(lines[1]).unwrap();
    let notice = notice["text"].as_str().unwrap();
    assert!(
        notice.starts_with("The background command `sh -c"),
        "{notice}"
    );
    let text = serde_json::to_string(notice).unwrap();
    assert_eq!(lines[1], format!("{{\"type\":\"notice\",\"text\":{text}}}"));
    assert_eq!(lines.len(), 2, "{lines:?}");
    let told = format!("Harness notice:\n{notice}");
    assert_eq!(last_prompts(&run.requests).last(), Some(&told));
    let question = |text: &str| PromptPayload {
        category: Some(PromptCategory::Question),
        classifier: GATE_HAIKU.to_string(),
        route: Route::turn(GATE_SONNET, RouteReason::Default).at_effort("medium"),
        led: Some(false),
        difficulty: Some(Difficulty::Medium),
        ..PromptPayload::new(text)
    };
    assert_eq!(run.prompts(), [question("fix the parser"), question(&told)]);
    assert_eq!(
        (run.report.agent_turns, run.report.result.as_str()),
        (2, "Noted.")
    );
}
