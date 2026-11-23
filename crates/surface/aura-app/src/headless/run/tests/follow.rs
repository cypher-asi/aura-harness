//! Gate's follow-up of work no check of its own vouched for: a change is
//! checked when its model is asked to, work that ran no check goes to
//! the judge, and the judge's verdict is the work's outcome.

use super::super::parse_session_id;
use super::{
    asked_on_sonnet, fix_the_parser, last_prompts, unfinished_provider, Routed, FIRST_CHANGE_NOTE,
};
use crate::headless::output::ExitReason;
use crate::headless::tests::{no_criteria, GATE_HAIKU, GATE_OPUS, GATE_SONNET};
use aura_core_types::{AgentId, Difficulty, OutcomeSignal, PromptCategory, PromptPayload};
use aura_model::{MockProvider, MockResponse};
use serde_json::{json, Value};

/// A change Sonnet made without a check: asked to check it, it runs
/// `cargo test`, which fails, the judge blames it, and Opus takes the
/// work over.
async fn checked_when_asked() -> Routed {
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
    fix_the_parser(provider).await
}

#[tokio::test]
async fn a_change_made_without_a_check_is_checked_before_the_turn_ends() {
    let run = checked_when_asked().await;

    let report = &run.report;
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
        run.models(),
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
    let nudge = run.requests.lock().unwrap()[4]
        .messages
        .last()
        .unwrap()
        .text_content();
    assert!(nudge.contains(aura_gate::followup::NUDGE_PROMPT), "{nudge}");
}

#[tokio::test]
async fn the_check_a_turn_is_asked_for_speaks_for_it() {
    let run = checked_when_asked().await;

    assert_eq!(
        run.signals(),
        [
            (GATE_SONNET, OutcomeSignal::ChecksFailed),
            (GATE_SONNET, OutcomeSignal::JudgedFail),
            (GATE_OPUS, OutcomeSignal::Unavailable),
        ],
        "the unchecked turn's outcome waits for the check it is asked for"
    );
    let attempts = run.attempts();
    let sonnet: Vec<_> = attempts.iter().filter(|a| a.model == GATE_SONNET).collect();
    assert_eq!(
        sonnet.len(),
        1,
        "the turn and the check it was asked for are logged once: {attempts:?}"
    );
    let first = &run.outcomes[0];
    assert_eq!(
        (sonnet[0].signal, sonnet[0].prompt_tx.as_str()),
        (
            OutcomeSignal::ChecksFailed,
            first.prompt_tx.to_hex().as_str()
        ),
        "the attempt's line says what its record says"
    );
    let usage = aura_model::Usage::new(100, 50);
    let price = aura_model::model_catalog::estimate_cost_usd(GATE_SONNET, &usage).unwrap();
    assert!(
        (first.cost_usd - 4.0 * price).abs() < 1e-9,
        "the check speaks for the turn that made the change, whose two calls it costs \
         along with its own two: {}",
        first.cost_usd
    );
    let ran: Vec<(&str, Option<Difficulty>)> = (attempts.iter())
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
    let run = fix_the_parser(provider).await;

    assert_eq!(
        (run.report.model.as_str(), run.report.result.as_str()),
        (GATE_SONNET, "Fixed the parser; `sh -n` passes.")
    );
    assert_eq!(
        run.models(),
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
    let requests = run.requests.lock().unwrap();
    let said = |message: &aura_model::Message| message.text_content();
    assert!((requests.iter().flat_map(|request| &request.messages))
        .all(|message| !aura_gate::evidence::transcript::said::is_nudge(&said(message))));
    let unverified = aura_gate::judge::turn::UNVERIFIED_SYSTEM_PROMPT;
    assert_eq!(requests[5].system.as_str(), unverified);
    assert_eq!(run.signals(), [(GATE_SONNET, OutcomeSignal::JudgedPass)]);
}

#[tokio::test]
async fn unchecked_work_that_looks_unfinished_goes_to_the_judge_then_moves_up() {
    let run = fix_the_parser(unfinished_provider()).await;

    assert_eq!(
        (run.report.model.as_str(), run.report.result.as_str()),
        (GATE_OPUS, "Fixed it.")
    );
    assert_eq!(
        run.models(),
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
        let requests = run.requests.lock().unwrap();
        let judge = &requests[3];
        let unverified = aura_gate::judge::turn::UNVERIFIED_SYSTEM_PROMPT;
        assert_eq!(judge.system.as_str(), unverified);
        let shown = judge.messages[0].text_content();
        assert!(shown.contains("fix the parser"), "{shown}");
        assert!(shown.contains("I could not find"), "{shown}");
    }
    let takeover = last_prompts(&run.requests).pop().unwrap();
    assert!(
        takeover.contains("It never opened the parser."),
        "the stronger model is told what the judge found: {takeover}"
    );
    assert_eq!(
        run.signals(),
        [
            (GATE_SONNET, OutcomeSignal::JudgedFail),
            (GATE_OPUS, OutcomeSignal::Unavailable),
        ],
        "the judge's verdict is the unchecked work's outcome"
    );
}

#[tokio::test]
async fn a_turn_whose_follow_up_changes_its_signal_is_recorded_once() {
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
    let run = fix_the_parser(provider).await;

    assert_eq!(run.report.model, GATE_OPUS, "{:?}", run.report.error);
    let signals = run.signals();
    assert_eq!(
        signals,
        [
            (GATE_SONNET, OutcomeSignal::JudgedFail),
            (GATE_OPUS, OutcomeSignal::Unavailable),
        ],
        "the check that passed is not recorded once the judge finds the work unfinished"
    );
    let attempts = run.attempts();
    let logged: Vec<(&str, OutcomeSignal)> = (attempts.iter())
        .map(|a| (a.model.as_str(), a.signal))
        .collect();
    assert_eq!(logged, signals, "each attempt is logged once, as recorded");
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

/// A run whose change Sonnet is asked to check, which it does not, and
/// whose unchecked work the judge then lets stand.
async fn nudged() -> Routed {
    let then = ["I read it again.", "It is done.\nVERDICT: OK"];
    fix_the_parser(unchecked_change_provider(&then)).await
}

#[tokio::test]
async fn a_nudge_streams_records_and_sends_its_message_byte_for_byte() {
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
    assert_eq!(run.linked(), [outcome]);
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
