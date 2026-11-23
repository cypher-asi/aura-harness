//! What a turn Gate routes records, shows and sends, byte for byte, when
//! its failed work moves to a stronger model, when the judge excuses the
//! failure, and when a route refuses the user's sign-in; and what a
//! `/compare` Gate run does when its work moves up.

use super::*;
use aura_core_types::{CompareStatus, Difficulty};
use aura_ui_terminal::events::MessageRole;

const HAIKU: &str = "aura-claude-haiku-4-5";
const SONNET: &str = "aura-claude-sonnet-4-6";

/// The line the criteria writer leaves when the mock answers it with a
/// command.
const NO_CRITERIA: &str = "gate: aura-claude-sonnet-4-6 (cloud) wrote no acceptance criteria: \
                           it called `run_command`, not the criteria tool";

/// The judge's line when it blames Haiku for a failed check.
const BLAMED: &str = "Gate's judge (aura-claude-opus-4-7 (cloud), $0.0018): the failure is \
                      aura-claude-haiku-4-5 (cloud)'s. The change is wrong.";

/// What Sonnet is told when it takes over Haiku's work after its check
/// failed and the judge blamed it.
const TAKEOVER_BLAMED: &str = concat!(
    "Harness notice:\nThe work above was done on aura-claude-haiku-4-5 (cloud) and ended with ",
    "its last test or build command failing. A reviewer of that work found: The change is ",
    "wrong. You are a stronger model taking over this turn. The files are as that work left ",
    "them: check what it changed, find what is wrong and finish the request."
);

/// An outcome as (index of its prompt, category, model, success, signal).
type Outcome = (usize, PromptCategory, String, Option<bool>, OutcomeSignal);

/// What a turn left: its prompts, its outcomes, the system lines its chat
/// showed and the user messages of its conversation.
struct Seen {
    prompts: Vec<PromptPayload>,
    outcomes: Vec<Outcome>,
    shown: Vec<String>,
    conversation: Vec<String>,
}

/// The system lines sent to `agent_id`'s chat so far.
fn system_lines(commands: &mut mpsc::Receiver<UiCommand>, agent_id: AgentId) -> Vec<String> {
    let hex = agent_id.to_hex();
    drain(commands)
        .into_iter()
        .filter(|(to, _)| to.as_deref() == Some(hex.as_str()))
        .filter_map(|(_, c)| match c {
            UiCommand::ShowMessage(m) if m.role == MessageRole::System => Some(m.content),
            _ => None,
        })
        .collect()
}

/// The prompts and outcomes in `entries`, each outcome naming its prompt
/// by index.
fn recorded(entries: &[aura_core_types::RecordEntry]) -> (Vec<PromptPayload>, Vec<Outcome>) {
    let prompts: Vec<(Hash, PromptPayload)> = entries
        .iter()
        .filter_map(|e| e.tx.prompt_payload().map(|p| (e.tx.hash, p)))
        .collect();
    let outcomes = entries
        .iter()
        .filter_map(|e| e.tx.outcome_payload())
        .map(|o| {
            let at = prompts.iter().position(|(hash, _)| *hash == o.prompt_tx);
            (at.unwrap(), o.category, o.model, o.success, o.signal)
        })
        .collect();
    (prompts.into_iter().map(|(_, p)| p).collect(), outcomes)
}

/// Run `text` as a turn of the first agent under Gate on zOS, after
/// `models`, if any, replaced the selection; returns what it left and
/// the models whose routes refused the sign-in.
async fn routed_turn(text: &str, models: Option<aura_auth::AuthSummary>) -> (Seen, Vec<String>) {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = match models {
        Some(auth) => ModelSelection::signed_in(
            &fixture.config,
            aura_model::model_catalog::GATE_MODEL_ID,
            auth,
        ),
        None => gate_on_zos(&fixture),
    };
    unrestricted(&mut state, &links, first).await;
    drain(&mut commands);
    let finished = turn(&mut state, &mut done, &links, first, text).await;
    let (prompts, outcomes) = recorded(&record_of(&state, first));
    let seen = Seen {
        prompts,
        outcomes,
        shown: system_lines(&mut commands, first),
        conversation: user_texts(&state, first),
    };
    (seen, finished.refused_models)
}

/// The debugging prompt `text`, as Gate records it when it starts the
/// turn on Haiku.
fn on_haiku(text: &str) -> PromptPayload {
    PromptPayload {
        category: Some(PromptCategory::Debugging),
        classifier: HAIKU.to_string(),
        route: Route::turn(HAIKU, RouteReason::Default).at_effort("medium"),
        led: Some(false),
        difficulty: Some(Difficulty::Easy),
        ..PromptPayload::new(text)
    }
}

/// The prompt Sonnet takes a debugging turn over with, told `text`.
fn taken_over(text: &str) -> PromptPayload {
    PromptPayload {
        category: Some(PromptCategory::Debugging),
        route: Route::turn(SONNET, RouteReason::Escalate).at_effort("medium"),
        ..PromptPayload::new(text)
    }
}

/// A turn that ends on Haiku or moves to Sonnet, and what it leaves
/// after the criteria writer's line and the first route's.
struct Case {
    text: &'static str,
    /// What Sonnet is told when it takes the turn over
    takeover: Option<&'static str>,
    outcomes: [Option<(&'static str, Option<bool>, OutcomeSignal)>; 3],
    shown: &'static [&'static str],
}

const ESCALATE: &str =
    "gate → aura-claude-sonnet-4-6 (cloud) at medium effort · debugging, escalate";

const CASES: [Case; 4] = [
    Case {
        text: "fix it (easy): run failing check (judge: blame)",
        takeover: Some(TAKEOVER_BLAMED),
        outcomes: [
            Some((HAIKU, Some(false), OutcomeSignal::ChecksFailed)),
            Some((HAIKU, Some(false), OutcomeSignal::JudgedFail)),
            Some((SONNET, None, OutcomeSignal::Unavailable)),
        ],
        shown: &[BLAMED, ESCALATE],
    },
    Case {
        text: "fix it (easy): run failing check (judge: excuse)",
        takeover: None,
        outcomes: [
            Some((HAIKU, Some(false), OutcomeSignal::ChecksFailed)),
            Some((HAIKU, None, OutcomeSignal::JudgedExcused)),
            None,
        ],
        shown: &[
            "Gate's judge (aura-claude-opus-4-7 (cloud), $0.0018): the failure is not \
                  aura-claude-haiku-4-5 (cloud)'s doing. The check cannot pass here.",
        ],
    },
    Case {
        text: "fix it (easy): run failing check",
        takeover: Some(concat!(
            "Harness notice:\nThe work above was done on aura-claude-haiku-4-5 (cloud) and ended ",
            "with its last test or build command failing. You are a stronger model taking over ",
            "this turn. The files are as that work left them: check what it changed, find what ",
            "is wrong and finish the request."
        )),
        outcomes: [
            Some((HAIKU, Some(false), OutcomeSignal::ChecksFailed)),
            Some((SONNET, None, OutcomeSignal::Unavailable)),
            None,
        ],
        shown: &[ESCALATE],
    },
    Case {
        text: "fix it (easy): run passing check, then reply unverified (review: blame)",
        takeover: Some(concat!(
            "Harness notice:\nThe work above was done on aura-claude-haiku-4-5 (cloud) and ended ",
            "without checking its work, which looked unfinished. A reviewer of that work found: ",
            "The change is untested and wrong. You are a stronger model taking over this turn. ",
            "The files are as that work left them: check what it changed, find what is wrong and ",
            "finish the request."
        )),
        outcomes: [
            Some((HAIKU, Some(false), OutcomeSignal::JudgedFail)),
            Some((SONNET, None, OutcomeSignal::Unavailable)),
            None,
        ],
        shown: &[
            "Gate's judge (aura-claude-sonnet-4-6 (cloud), $0.0010): aura-claude-haiku-4-5 \
             (cloud)'s unchecked work is not done. The change is untested and wrong.",
            ESCALATE,
        ],
    },
];

#[tokio::test]
async fn failed_work_records_shows_and_sends_its_move_up_byte_for_byte() {
    for case in &CASES {
        let (seen, refused) = routed_turn(case.text, None).await;
        let mut prompts = vec![on_haiku(case.text)];
        prompts.extend(case.takeover.map(taken_over));
        assert_eq!(seen.prompts, prompts, "{}", case.text);
        let outcomes: Vec<Outcome> = (case.outcomes.iter().flatten())
            .map(|&(model, success, signal)| {
                let at = usize::from(model == SONNET);
                let category = PromptCategory::Debugging;
                (at, category, model.to_string(), success, signal)
            })
            .collect();
        assert_eq!(seen.outcomes, outcomes, "{}", case.text);
        let route = "gate → aura-claude-haiku-4-5 (cloud) at medium effort · debugging, default";
        let mut shown = vec![NO_CRITERIA, route];
        shown.extend(case.shown);
        assert_eq!(seen.shown, shown, "{}", case.text);
        let mut conversation = vec![case.text, ""];
        conversation.extend(case.takeover);
        assert_eq!(seen.conversation, conversation, "{}", case.text);
        assert!(refused.is_empty());
    }
}

#[tokio::test]
async fn a_refused_sign_in_records_shows_and_sends_its_reroute_byte_for_byte() {
    let claude_plan = aura_auth::AuthSummary {
        subscriptions: vec![aura_auth::ProviderKind::Claude],
        ..aura_auth::AuthSummary::default()
    };
    let (seen, refused) = routed_turn("hello", Some(claude_plan)).await;

    let asked = PromptPayload {
        category: Some(PromptCategory::Question),
        classifier: "claude/claude-haiku-4-5".to_string(),
        route: Route::turn(REFUSING_MODEL, RouteReason::Default).at_effort("medium"),
        led: Some(false),
        difficulty: Some(Difficulty::Medium),
        ..PromptPayload::new("hello")
    };
    assert_eq!(seen.prompts, [asked]);
    let opus = "claude/claude-opus-4-7".to_string();
    let outcome = (
        0,
        PromptCategory::Question,
        opus,
        None,
        OutcomeSignal::Unavailable,
    );
    assert_eq!(seen.outcomes, [outcome]);
    assert_eq!(
        seen.shown,
        [
            "gate → claude-sonnet-4-6 (claude plan) at medium effort · question, default",
            "claude/claude-sonnet-4-6 refused your sign-in; Gate skips it until your sign-ins \
             change.\ngate → claude-opus-4-7 (claude plan) at medium effort · question, default",
        ]
    );
    assert_eq!(seen.conversation, ["hello"]);
    assert_eq!(refused, [REFUSING_MODEL]);
}

#[tokio::test]
async fn a_compare_gate_run_records_shows_and_sends_its_move_up_byte_for_byte() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    let text = "fix it (easy): run failing check (judge: blame)";
    let args = format!("{text} --repeats 1 --against opus");
    let payload = compare_runs::run_comparison(&mut state, &links, &mut commands, &args).await;
    assert_eq!(payload.status, CompareStatus::Done, "{payload:#?}");
    let run = payload.run("a-1").unwrap();

    let (prompts, outcomes) = recorded(&compare_runs::entries(&state, run.agent_id));
    let first = PromptPayload {
        route: Route::turn(HAIKU, RouteReason::Compare).at_effort("medium"),
        ..on_haiku(text)
    };
    let takeover = PromptPayload {
        classifier: HAIKU.to_string(),
        ..taken_over(TAKEOVER_BLAMED)
    };
    assert_eq!(prompts, [first, takeover]);
    let debugging = PromptCategory::Debugging;
    let failed = |signal| (0, debugging, HAIKU.to_string(), Some(false), signal);
    let moved = (
        1,
        debugging,
        SONNET.to_string(),
        None,
        OutcomeSignal::Unavailable,
    );
    let signals = [OutcomeSignal::ChecksFailed, OutcomeSignal::JudgedFail];
    assert_eq!(outcomes, [failed(signals[0]), failed(signals[1]), moved]);
    assert_eq!(
        system_lines(&mut commands, run.agent_id),
        [
            NO_CRITERIA,
            "Compare #1 a-1 · gate:nodelegate → aura-claude-haiku-4-5 (cloud) · debugging",
            BLAMED,
            "Compare #1 a-1 · gate → aura-claude-sonnet-4-6 (cloud) at medium effort · \
             debugging, escalate",
        ]
    );
    assert_eq!(
        parked_conversation(&state, run.agent_id),
        [text, "", TAKEOVER_BLAMED]
    );
}

/// The user messages of the session `agent_id`'s run parked when it
/// ended.
fn parked_conversation(state: &LoopState<'_>, agent_id: AgentId) -> Vec<String> {
    let session = (state.hub.take_finished().into_iter())
        .find(|session| session.agent_id() == agent_id)
        .unwrap();
    (session.messages.iter())
        .filter(|m| m.role == aura_model::Role::User)
        .map(aura_model::Message::text_content)
        .collect()
}
