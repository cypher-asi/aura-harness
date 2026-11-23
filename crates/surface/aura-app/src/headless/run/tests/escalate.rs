//! A failed turn: Gate's judge says whether its model was at fault, and
//! unless it excuses the failure a stronger model takes the turn over,
//! told what the judge found, under a prompt of its own.

use super::{
    asked_on_sonnet, fix_the_parser, last_prompts, route_run, unfinished_provider, Routed,
    FIRST_CHANGE_NOTE,
};
use crate::headless::output::ExitReason;
use crate::headless::tests::{
    failing_check_provider, no_criteria, sonnet_and_opus, FAILS, GATE_HAIKU, GATE_OPUS, GATE_SONNET,
};
use aura_core_types::{OutcomeSignal, PromptCategory, PromptPayload, Route, RouteReason};
use aura_model::{MockProvider, MockResponse};
use serde_json::{json, Value};
use std::path::Path;

/// Run "fix the parser" on [`failing_check_provider`], answered `then`
/// once its check failed.
async fn failed(then: &[&str]) -> Routed {
    fix_the_parser(failing_check_provider(then)).await
}

/// A failed check the judge blames on Sonnet, then Opus fixing it.
async fn blamed() -> Routed {
    failed(&[
        "The parser still drops the last token.\nVERDICT: MODEL",
        "Fixed it.",
    ])
    .await
}

/// The `judged` lines `run` streamed, read.
fn judged(run: &Routed) -> Vec<Value> {
    (run.gate_events().into_iter())
        .filter(|line| line["type"] == "judged")
        .collect()
}

/// The prompt Opus takes a debugging turn over with, told `text`.
fn taken_over(text: String) -> PromptPayload {
    PromptPayload {
        category: Some(PromptCategory::Debugging),
        route: Route::turn(GATE_OPUS, RouteReason::Escalate).at_effort("medium"),
        led: Some(false),
        ..PromptPayload::new(text)
    }
}

#[tokio::test]
async fn a_failed_turn_the_judge_blames_on_the_model_goes_on_with_a_stronger_one() {
    let run = blamed().await;

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
        run.models(),
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
    let route = |model, reason| {
        let route = Route::turn(model, reason).at_effort("medium");
        (Some(PromptCategory::Debugging), route)
    };
    let (default, escalate) = (RouteReason::Default, RouteReason::Escalate);
    let routes = [route(GATE_SONNET, default), route(GATE_OPUS, escalate)];
    assert_eq!(run.routes(), routes);
    assert_eq!(
        run.signals(),
        [
            (GATE_SONNET, OutcomeSignal::ChecksFailed),
            (GATE_SONNET, OutcomeSignal::JudgedFail),
            (GATE_OPUS, OutcomeSignal::Unavailable),
        ]
    );
    assert_eq!(
        [run.row(GATE_SONNET), run.row(GATE_OPUS)],
        [Some((1, 0)), None],
        "the failure counts once for Sonnet; the stronger model's turn is not counted"
    );
}

#[tokio::test]
async fn the_judge_sees_the_failed_turn_and_the_stronger_model_what_it_found() {
    let run = blamed().await;

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
    let judged = judged(&run);
    assert_eq!(judged.len(), 1, "{:?}", run.printed);
    assert_eq!(judged[0]["model"], GATE_SONNET);
    assert_eq!(judged[0]["judge"], GATE_OPUS);
    assert_eq!(judged[0]["at_fault"], true);
}

#[tokio::test]
async fn a_failed_turn_the_judge_excuses_stays_put_and_counts_for_nothing() {
    let run = failed(&["There is no crate here to test.\nVERDICT: OTHER"]).await;

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
        run.models(),
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
    let judged = judged(&run);
    assert_eq!(judged.len(), 1, "{:?}", run.printed);
    assert_eq!(judged[0]["at_fault"], false);
    assert_eq!(judged[0]["reasons"], "There is no crate here to test.");
    let on_sonnet = Route::turn(GATE_SONNET, RouteReason::Default).at_effort("medium");
    assert_eq!(run.routes(), [(Some(PromptCategory::Debugging), on_sonnet)]);
    assert_eq!(
        run.signals(),
        [
            (GATE_SONNET, OutcomeSignal::ChecksFailed),
            (GATE_SONNET, OutcomeSignal::JudgedExcused),
        ]
    );
    assert_eq!(
        [run.row(GATE_SONNET), run.row(GATE_OPUS)],
        [None, None],
        "a failure that was not the model's doing counts for nothing"
    );
}

#[tokio::test]
async fn a_failed_turn_the_judge_says_nothing_clear_about_still_moves_up() {
    let run = failed(&["Hard to say.", "Fixed it."]).await;

    assert_eq!(run.report.model, GATE_OPUS);
    assert_eq!(run.report.result, "Fixed it.");
    assert!(judged(&run).is_empty());
    assert_eq!(
        run.signals(),
        [
            (GATE_SONNET, OutcomeSignal::ChecksFailed),
            (GATE_OPUS, OutcomeSignal::Unavailable),
        ],
        "the turn's own signal stands"
    );
    assert_eq!(
        [run.row(GATE_SONNET), run.row(GATE_OPUS)],
        [Some((1, 0)), None]
    );
}

/// What Opus is told when it takes over Sonnet's work after its last
/// check failed and the judge blamed it.
const TAKEOVER_BLAMED: &str = concat!(
    "The work above was done on aura-claude-sonnet-4-6 (cloud) and ended with its last ",
    "test or build command failing. A reviewer of that work found: The parser still ",
    "drops the last token. You are a stronger model taking over this turn. The files ",
    "are as that work left them: check what it changed, find what is wrong and finish ",
    "the request."
);

/// What Opus is told when it takes over Sonnet's unchecked work, which
/// the judge found unfinished.
const TAKEOVER_UNFINISHED: &str = concat!(
    "The work above was done on aura-claude-sonnet-4-6 (cloud) and ended without ",
    "checking its work, which looked unfinished. A reviewer of that work found: It ",
    "never opened the parser. You are a stronger model taking over this turn. The ",
    "files are as that work left them: check what it changed, find what is wrong and ",
    "finish the request."
);

/// A run that may move up: its provider, the `judged` line it streams,
/// what Opus is told when it takes the work over (`None` when it does
/// not), and the user messages of the turn before.
type MoveUp = (MockProvider, Value, Option<&'static str>, Vec<&'static str>);

/// A failed check the judge blames, one it excuses, and unchecked work
/// it finds unfinished.
fn move_ups() -> [MoveUp; 3] {
    let failed = |verdict| failing_check_provider(&[verdict, "Fixed it."]);
    let judged = |judge: &str, at_fault: bool, cost_usd: f64, reasons: &str| {
        json!({"type": "judged", "model": GATE_SONNET, "judge": judge, "at_fault": at_fault,
               "reasons": reasons, "cost_usd": cost_usd})
    };
    let blamed = "The parser still drops the last token.";
    let excused = "There is no crate here to test.";
    let unfinished = "It never opened the parser.";
    [
        (
            failed("The parser still drops the last token.\nVERDICT: MODEL"),
            judged(GATE_OPUS, true, 0.00175, blamed),
            Some(TAKEOVER_BLAMED),
            vec!["fix the parser", FIRST_CHANGE_NOTE],
        ),
        (
            failed("There is no crate here to test.\nVERDICT: OTHER"),
            judged(GATE_OPUS, false, 0.00175, excused),
            None,
            vec!["fix the parser", FIRST_CHANGE_NOTE],
        ),
        (
            unfinished_provider(),
            judged(GATE_SONNET, true, 0.00105, unfinished),
            Some(TAKEOVER_UNFINISHED),
            vec!["fix the parser"],
        ),
    ]
}

#[tokio::test]
async fn a_move_up_streams_records_and_sends_its_takeover_byte_for_byte() {
    for (provider, judged, takeover, mut sent) in move_ups() {
        let run = fix_the_parser(provider).await;
        let mut streamed = vec![judged];
        streamed.extend(takeover.map(|text| json!({"type": "notice", "text": text})));
        assert_eq!(run.gate_events(), streamed);
        let told = takeover.map(|text| format!("Harness notice:\n{text}"));
        let mut recorded = vec![asked_on_sonnet()];
        recorded.extend(told.clone().map(taken_over));
        assert_eq!(run.prompts(), recorded);
        sent.extend(told.as_deref());
        assert_eq!(last_prompts(&run.requests), sent);
    }
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
    let provider = failing_script_provider();
    let setup = |workspace: &Path, _: &_| failing_script(workspace);
    let run = route_run(sonnet_and_opus(), provider, "fix the parser", setup).await;

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

/// A model whose prompt is classified as debugging, with no criteria to
/// check it against, and whose turn's call fails, which the judge blames
/// on it; Opus then fixes the parser.
fn failing_call_provider() -> MockProvider {
    MockProvider::new()
        .with_response(MockResponse::text("debugging medium"))
        .with_response(no_criteria())
        .with_response(MockResponse::text(FAILS))
        .with_response(MockResponse::text("It broke.\nVERDICT: MODEL"))
        .with_response(MockResponse::text("Fixed it."))
}

/// What Opus is told when it takes over Sonnet's work after its model
/// call failed and the judge blamed it.
const TAKEOVER_ERRED: &str = "The work above was done on aura-claude-sonnet-4-6 (cloud) and \
                              ended in an error. A reviewer of that work found: It broke. You \
                              are a stronger model taking over this turn. The files are as that \
                              work left them: check what it changed, find what is wrong and \
                              finish the request.";

#[tokio::test]
async fn a_model_error_moves_the_turn_up_byte_for_byte() {
    let run = fix_the_parser(failing_call_provider()).await;

    let report = &run.report;
    assert_eq!(
        (report.exit_reason, report.error.as_deref()),
        (ExitReason::Success, None)
    );
    assert_eq!(
        (report.model.as_str(), report.result.as_str()),
        (GATE_OPUS, "Fixed it.")
    );
    let judged = "{\"type\":\"judged\",\"model\":\"aura-claude-sonnet-4-6\",\"judge\":\
                  \"aura-claude-opus-4-7\",\"at_fault\":true,\"reasons\":\"It broke.\",\
                  \"cost_usd\":0.00175}";
    let notice = format!("{{\"type\":\"notice\",\"text\":\"{TAKEOVER_ERRED}\"}}");
    assert_eq!(run.gate_lines(), [judged, &notice]);
    let takeover = taken_over(format!("Harness notice:\n{TAKEOVER_ERRED}"));
    assert_eq!(run.prompts(), [asked_on_sonnet(), takeover]);
    let outcome = |at, model: &str, success, signal, cost_usd| {
        let model = model.to_string();
        (
            at,
            PromptCategory::Debugging,
            model,
            success,
            signal,
            cost_usd,
        )
    };
    assert_eq!(
        run.linked(),
        [
            outcome(0, GATE_SONNET, Some(false), OutcomeSignal::Error, 0.0),
            outcome(0, GATE_SONNET, Some(false), OutcomeSignal::JudgedFail, 0.0),
            outcome(1, GATE_OPUS, None, OutcomeSignal::Unavailable, 0.00175),
        ]
    );
}
