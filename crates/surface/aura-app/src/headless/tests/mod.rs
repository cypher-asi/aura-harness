//! The harness `aura -p`'s tests run on, which the run's own tests (in
//! `run/tests`) share, and the tests of headless's other parts.
//!
//! - `tools`: the tools a run offers, and what its mode lets them do
//! - `eval`: `aura eval run`'s runs under Gate
//! - `output`: what Gate's own check streams

#![deny(clippy::too_many_lines)]

mod eval;
mod output;
mod tools;

use super::output::{Output, RunReport};
use super::run::{Headless, Request, RunLimits, SessionChoice};
use super::tools::ToolFilter;
use super::OutputFormat;
use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::event_loop::{AgentHub, GateBase, Permissions, TurnBase};
use crate::harness::{
    agent_tools, background_process_manager, build_hub, mode_policy, policy_tool_names, Harness,
    HubSetup,
};
use crate::long_run::LongRunSettings;
use crate::session_helpers::default_agent_config_with_auth;
use async_trait::async_trait;
use aura_core_types::{AgentId, RecordEntry, Transaction};
use aura_gate::route::Candidate;
use aura_model::model_catalog::{ModelBackend, ModelTier};
use aura_model::{
    MockProvider, MockResponse, ModelProvider, ModelRequest, ModelResponse, ReasonerError,
};
use aura_store_db::RocksStore;
use serde_json::{json, Value};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Every request a run's model was sent, Gate's included, in order.
pub(super) type Requests = Arc<Mutex<Vec<ModelRequest>>>;

/// A mock model that also keeps every request it was sent. Calls are
/// billed as the model asked for, so calls to a catalog model have a
/// list price.
struct Recording {
    inner: MockProvider,
    requests: Requests,
}

/// A reply [`Recording`] turns into a failed call, as from a model that
/// fell over.
pub(super) const FAILS: &str = "<the call fails>";

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
pub(super) struct Captured(Arc<Mutex<Vec<u8>>>);

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
    pub(super) fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }

    pub(super) fn json_lines(&self) -> Vec<Value> {
        self.text()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

pub(super) struct Dirs {
    pub(super) data: TempDir,
    pub(super) workspace: TempDir,
}

pub(super) fn dirs() -> Dirs {
    Dirs {
        data: TempDir::new().unwrap(),
        workspace: TempDir::new().unwrap(),
    }
}

/// A headless harness on `dirs` whose model answers with `provider`.
pub(super) fn headless(
    dirs: &Dirs,
    provider: MockProvider,
    filter: ToolFilter,
) -> (Headless, Requests) {
    let permissions = super::permissions(aura_core_modes::AgentMode::Agent, None);
    headless_in(dirs, provider, filter, permissions)
}

/// [`headless`] with its agents under `permissions`.
pub(super) fn headless_in(
    dirs: &Dirs,
    provider: MockProvider,
    filter: ToolFilter,
    permissions: Permissions,
) -> (Headless, Requests) {
    harness(dirs, provider, filter, permissions, GateBase::default())
}

/// [`headless_in`] with Gate classifying and routing as `gate` says.
pub(super) fn harness(
    dirs: &Dirs,
    provider: MockProvider,
    filter: ToolFilter,
    permissions: Permissions,
    gate: GateBase,
) -> (Headless, Requests) {
    let requests = Requests::default();
    let provider = Arc::new(Recording {
        inner: provider,
        requests: Arc::clone(&requests),
    });
    let (setup, completions) = hub_setup(dirs, provider, &filter, permissions, gate);
    let Harness { hub, background } = build_hub(setup);
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
        workspace_root: dirs.workspace.path().to_path_buf(),
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

/// How [`harness`] builds its hub on `dirs`, the model answering with
/// `provider`, and where its background commands report that they
/// ended.
fn hub_setup(
    dirs: &Dirs,
    provider: Arc<Recording>,
    filter: &ToolFilter,
    permissions: Permissions,
    gate: GateBase,
) -> (HubSetup, mpsc::Receiver<Transaction>) {
    let data = dirs.data.path();
    let workspace = dirs.workspace.path().to_path_buf();
    let (tool_config, tools, web_tools) = agent_tools(false);
    let tool_names = policy_tool_names(&tools);
    let config = default_agent_config_with_auth("mock-model", None);
    let (commands, mut ui) = mpsc::channel(256);
    tokio::spawn(async move { while ui.recv().await.is_some() {} });
    let (process_manager, completions) = background_process_manager();
    let store = Arc::new(RocksStore::open(data.join("db"), false).unwrap());
    let registry = AgentRegistry::open(
        aura_store_db::AgentStore::for_store(&store),
        data,
        &workspace,
    )
    .unwrap();
    let setup = HubSetup {
        data_dir: data.to_path_buf(),
        workspace_root: workspace.clone(),
        registry,
        definitions: AgentDefinitions::load_or_seed(data).unwrap(),
        store,
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
    };
    (setup, completions)
}

pub(super) fn request(prompt: &str, session: SessionChoice) -> Request {
    Request {
        prompt: prompt.to_string(),
        session,
        limits: RunLimits::default(),
    }
}

pub(super) async fn execute(
    headless: &mut Headless,
    request: Request,
    format: OutputFormat,
) -> (RunReport, Captured) {
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

/// The model of each request in `requests`, in order.
pub(super) fn models(requests: &Requests) -> Vec<String> {
    (requests.lock().unwrap().iter())
        .map(|r| r.model.to_string())
        .collect()
}

/// The record of `agent` in `hub`'s store.
pub(super) fn record_of(hub: &AgentHub, agent: AgentId) -> Vec<RecordEntry> {
    hub.factory.store.scan_record(agent, 1, 1_000).unwrap()
}

pub(super) const GATE_HAIKU: &str = "aura-claude-haiku-4-5";
pub(super) const GATE_SONNET: &str = "aura-claude-sonnet-4-6";
pub(super) const GATE_OPUS: &str = "aura-claude-opus-4-7";

fn candidate(id: &str, tier: ModelTier, cost_usd: f64) -> Candidate {
    Candidate {
        id: id.to_string(),
        label: id.to_string(),
        backend: ModelBackend::Router,
        tier,
        cost_usd: Some(cost_usd),
    }
}

/// Gate routing to Sonnet alone, classifying with Haiku, so runs never
/// explore.
pub(super) fn sonnet_only() -> GateBase {
    GateBase {
        classifier: GATE_HAIKU.to_string(),
        candidates: Some(vec![candidate(GATE_SONNET, ModelTier::Standard, 0.1)]),
    }
}

/// Gate routing among Sonnet and Opus, classifying with Haiku. Debugging
/// starts on Sonnet, the cheapest of the two, so runs never explore.
pub(super) fn sonnet_and_opus() -> GateBase {
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
pub(super) fn no_criteria() -> MockResponse {
    MockResponse::text("No criteria.")
}

/// A model whose prompt is classified as debugging, with no criteria to
/// check it against, and whose turn writes `parser.txt`, then runs
/// `cargo test` (which fails in a workspace with no crate) and gives up.
/// The model calls after that are answered with `then`, in order.
pub(super) fn failing_check_provider(then: &[&str]) -> MockProvider {
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
