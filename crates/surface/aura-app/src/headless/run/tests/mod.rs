//! The tests of a headless run, on the harness of `headless`'s tests:
//! what it prints and streams, the sessions it picks up and the limits
//! that stop it, and what it sends, streams, records and prints when
//! Gate routes its turns.
//!
//! - `session`: session ids, and the sessions `--resume` and
//!   `--continue` pick up
//! - `report`: what a run prints and streams in each output format
//! - `limits`: the limits that stop a run
//! - `route`: how Gate routes a run's turns, and what it records of them
//! - `follow`: Gate's follow-up of work no check of its own vouched for
//! - `escalate`: a failed turn, Gate's judge of it, and the move to a
//!   stronger model

#![deny(clippy::too_many_lines)]

mod escalate;
mod follow;
mod limits;
mod report;
mod route;
mod session;

use super::{parse_session_id, Headless, SessionChoice};
use crate::event_loop::GateBase;
use crate::headless::output::RunReport;
use crate::headless::tests::{
    dirs, execute, harness, models, record_of, request, sonnet_and_opus, Requests, GATE_HAIKU,
    GATE_SONNET,
};
use crate::headless::tools::ToolFilter;
use crate::headless::{permissions, OutputFormat};
use aura_core_modes::AgentMode;
use aura_core_types::{AgentId, Difficulty, OutcomePayload, OutcomeSignal, PromptCategory};
use aura_core_types::{PromptPayload, RecordEntry, Route, RouteLevel, RouteReason};
use aura_gate::attempts::{Attempt, AttemptLog, Line};
use aura_gate::route::stats::GateStats;
use aura_model::{MockProvider, MockResponse, Role};
use serde_json::{json, Value};
use std::path::Path;

/// A model call that writes the todo list.
fn todo_call() -> MockResponse {
    MockResponse::tool_use(
        "t1",
        "todo_write",
        json!({"todos": [{"id": "1", "content": "Write the parser", "status": "completed"}]}),
    )
}

/// User prompts the model was sent in the last request of an agent turn,
/// not one of Gate's own calls.
fn last_prompts(requests: &Requests) -> Vec<String> {
    let requests = requests.lock().unwrap();
    requests
        .iter()
        .rev()
        .find(|request| request.metadata.purpose.is_none())
        .unwrap()
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .map(aura_model::Message::text_content)
        .filter(|t| !t.is_empty())
        .collect()
}

/// What the agent loop tells the model once it has changed its first
/// file.
const FIRST_CHANGE_NOTE: &str = "NOTE: You've made your first file change. Before making more \
                                 changes, consider verifying your work (e.g., run the build or \
                                 tests) to catch issues early.";

/// How each attempt line of Gate's attempt log starts.
const ATTEMPT_LINE: &str = r#"{"kind":"attempt","#;

/// What a run Gate routed left: its report, the lines it printed, every
/// request its model was sent, its record and the outcomes in it, the
/// attempt lines of Gate's attempt log as they were written, what the
/// run showed before its first turn, and Gate's table and recent routes
/// after it.
struct Routed {
    report: RunReport,
    printed: Vec<String>,
    requests: Requests,
    entries: Vec<RecordEntry>,
    outcomes: Vec<OutcomePayload>,
    attempts: Vec<String>,
    setup: Vec<String>,
    stats: GateStats,
    recent: usize,
}

/// An outcome as (index of its prompt, category, model, success, signal,
/// cost in USD).
type Outcome = (
    usize,
    PromptCategory,
    String,
    Option<bool>,
    OutcomeSignal,
    f64,
);

/// Run `prompt` under `gate` on `provider`, streaming, once `setup` has
/// prepared the run's workspace and its harness.
async fn route_run(
    gate: GateBase,
    provider: MockProvider,
    prompt: &str,
    setup: impl FnOnce(&Path, &Headless),
) -> Routed {
    let dirs = dirs();
    let permissions = permissions(AgentMode::Agent, None);
    let (mut h, requests) = harness(&dirs, provider, ToolFilter::default(), permissions, gate);
    setup(dirs.workspace.path(), &h);
    let asked = request(prompt, SessionChoice::New);
    let (report, captured) = execute(&mut h, asked, OutputFormat::StreamJson).await;
    let (agent, _) = parse_session_id(&report.session_id, AgentId::generate()).unwrap();
    let log = AttemptLog::in_data_dir(dirs.data.path());
    let logged = std::fs::read_to_string(log.path()).unwrap_or_default();
    let entries = record_of(&h.hub, agent);
    Routed {
        printed: captured.text().lines().map(str::to_string).collect(),
        requests,
        outcomes: (entries.iter())
            .filter_map(|e| e.tx.outcome_payload())
            .collect(),
        entries,
        attempts: (logged.lines().filter(|line| line.starts_with(ATTEMPT_LINE)))
            .map(str::to_string)
            .collect(),
        setup: h.setup_lines(),
        stats: h.hub.gate.stats(),
        recent: h.hub.gate.recent().len(),
        report,
    }
}

/// Run "fix the parser" under [`sonnet_and_opus`] on `provider`.
async fn fix_the_parser(provider: MockProvider) -> Routed {
    route_run(sonnet_and_opus(), provider, "fix the parser", |_, _| {}).await
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

    /// [`Self::gate_lines`], read.
    fn gate_events(&self) -> Vec<Value> {
        (self.gate_lines().into_iter())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// The model of each model call, in order.
    fn models(&self) -> Vec<String> {
        models(&self.requests)
    }

    /// The prompts it recorded.
    fn prompts(&self) -> Vec<PromptPayload> {
        (self.entries.iter())
            .filter_map(|e| e.tx.prompt_payload())
            .collect()
    }

    /// The category and route of each prompt it recorded.
    fn routes(&self) -> Vec<(Option<PromptCategory>, Route)> {
        (self.prompts().into_iter())
            .map(|p| (p.category, p.route))
            .collect()
    }

    /// The model and signal of each outcome it recorded.
    fn signals(&self) -> Vec<(&str, OutcomeSignal)> {
        (self.outcomes.iter())
            .map(|o| (o.model.as_str(), o.signal))
            .collect()
    }

    /// The outcomes it recorded, each naming its prompt by index.
    fn linked(&self) -> Vec<Outcome> {
        let prompts: Vec<_> = (self.entries.iter())
            .filter(|e| e.tx.prompt_payload().is_some())
            .map(|e| e.tx.hash)
            .collect();
        (self.outcomes.iter())
            .map(|o| {
                let at = prompts.iter().position(|hash| *hash == o.prompt_tx);
                let model = o.model.clone();
                (
                    at.unwrap(),
                    o.category,
                    model,
                    o.success,
                    o.signal,
                    o.cost_usd,
                )
            })
            .collect()
    }

    /// The attempts of Gate's attempt log.
    fn attempts(&self) -> Vec<Attempt> {
        (self.attempts.iter())
            .filter_map(|line| match serde_json::from_str(line).unwrap() {
                Line::Attempt(attempt) => Some(attempt),
                _ => None,
            })
            .collect()
    }

    /// Tries and successes of `model`'s debugging row in Gate's table.
    fn row(&self, model: &str) -> Option<(u32, u32)> {
        (self.stats)
            .row(RouteLevel::Turn, PromptCategory::Debugging, model)
            .map(|row| (row.tries, row.successes))
    }
}

/// The prompt of "fix the parser" as Gate records it when the turn starts
/// on Sonnet.
fn asked_on_sonnet() -> PromptPayload {
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
/// check it against, whose turn says it could not find the bug, which
/// the judge blames on it, and whose stronger model then fixes it.
fn unfinished_provider() -> MockProvider {
    MockProvider::new()
        .with_response(MockResponse::text("debugging medium"))
        .with_response(crate::headless::tests::no_criteria())
        .with_response(MockResponse::text(
            "I could not find where the parser drops the token.",
        ))
        .with_response(MockResponse::text(
            "It never opened the parser.\nVERDICT: MODEL",
        ))
        .with_response(MockResponse::text("Fixed it."))
}
