//! How Gate routes a run's turns, and what it records of them: the
//! prompt classified and routed, the turn led when Gate's table says so,
//! the criteria the work is held to, and the notices that start more
//! turns.

use super::{asked_on_sonnet, fix_the_parser, last_prompts, route_run, Routed};
use crate::headless::output::ExitReason;
use crate::headless::tests::{sonnet_only, GATE_HAIKU, GATE_SONNET};
use aura_core_types::{Difficulty, OutcomeSignal, PromptCategory, PromptPayload};
use aura_core_types::{Route, RouteLevel, RouteReason};
use aura_model::{MockProvider, MockResponse};
use serde_json::{json, Value};

/// A question Gate routes to Sonnet, the one model it routes among.
async fn a_question() -> Routed {
    let provider = MockProvider::simple_response("question medium");
    route_run(sonnet_only(), provider, "what is a monad?", |_, _| {}).await
}

#[tokio::test]
async fn gate_routes_headless_turns_and_records_how_they_went() {
    let run = a_question().await;

    assert_eq!(run.report.exit_reason, ExitReason::Success);
    assert_eq!(run.report.model, GATE_SONNET);
    assert_eq!(
        run.models(),
        [GATE_HAIKU, GATE_SONNET, GATE_SONNET],
        "classified, routed, then judged, as the turn ran no check"
    );
    assert_eq!(
        run.setup,
        [
            format!("gate: prompts classified by {GATE_HAIKU} (cloud)"),
            format!("gate: routing among {GATE_SONNET} (cloud)"),
        ]
    );
    let prompts = run.prompts();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].category, Some(PromptCategory::Question));
    assert_eq!(prompts[0].classifier, GATE_HAIKU);
    assert_eq!(
        prompts[0].route,
        Route::turn(GATE_SONNET, RouteReason::Default).at_effort("medium")
    );
    assert_eq!(run.signals(), [(GATE_SONNET, OutcomeSignal::Unavailable)]);
    assert_eq!(run.recent, 1);
}

#[tokio::test]
async fn a_routed_question_is_answered_by_the_model_it_is_routed_to() {
    let run = a_question().await;

    let requests = run.requests.lock().unwrap();
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

#[tokio::test]
async fn a_routed_runs_cost_counts_gates_calls() {
    let run = a_question().await;

    let report = &run.report;
    let price = |model: &str| {
        aura_model::model_catalog::estimate_cost_usd(model, &aura_model::Usage::new(100, 50))
            .unwrap()
    };
    let about =
        |cost: Option<f64>, expected: f64| cost.is_some_and(|c| (c - expected).abs() < 1e-9);
    assert!(
        about(report.usage.cost_usd, price(GATE_SONNET)),
        "the agent's call: {:?}",
        report.usage
    );
    assert!(
        about(report.gate_cost_usd, price(GATE_HAIKU) + price(GATE_SONNET)),
        "the classifier's call and the judge's: {:?}",
        report.gate_cost_usd
    );
    assert!(
        about(
            report.total_cost_usd,
            price(GATE_HAIKU) + 2.0 * price(GATE_SONNET)
        ),
        "the run's cost counts Gate's calls: {:?}",
        report.total_cost_usd
    );
}

/// A change to several files Gate routes to Sonnet, whose table says
/// such changes go better led.
async fn led() -> Routed {
    let provider = MockProvider::simple_response("multi_file_change medium");
    let work = (PromptCategory::MultiFileChange, Some(Difficulty::Medium));
    route_run(
        sonnet_only(),
        provider,
        "rename the crate everywhere",
        |_, h| {
            for (led, outcomes) in [(true, (10, true)), (false, (3, false))] {
                (h.hub.gate).learn(RouteLevel::Turn, work, GATE_SONNET, Some(led), outcomes);
            }
        },
    )
    .await
}

#[tokio::test]
async fn a_headless_turn_changing_several_files_is_led() {
    let run = led().await;

    assert_eq!(run.report.exit_reason, ExitReason::Success);
    let requests = run.requests.lock().unwrap();
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
async fn a_led_turn_records_its_prompt_as_led() {
    let run = led().await;

    let led = PromptPayload {
        category: Some(PromptCategory::MultiFileChange),
        classifier: GATE_HAIKU.to_string(),
        route: Route::turn(GATE_SONNET, RouteReason::Default).at_effort("medium"),
        led: Some(true),
        difficulty: Some(Difficulty::Medium),
        ..PromptPayload::new("rename the crate everywhere")
    };
    assert_eq!(run.prompts(), [led]);
    let unavailable = OutcomeSignal::Unavailable;
    let linked: Vec<_> = (run.linked().into_iter())
        .map(|(at, _, model, _, signal, _)| (at, model, signal))
        .collect();
    assert_eq!(linked, [(0, GATE_SONNET.to_string(), unavailable)]);
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
    let run = fix_the_parser(provider).await;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_notice_starts_a_routed_turn_byte_for_byte() {
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
    let run = fix_the_parser(provider).await;

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
