//! What turns record: routes, prompts and outcomes.

use super::*;

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

/// Models for `fixture` signed in to zOS as alice, with Gate picking.
fn signed_in_on_gate(fixture: &Fixture) -> ModelSelection {
    ModelSelection::signed_in(
        &fixture.config,
        aura_model::model_catalog::GATE_MODEL_ID,
        aura_auth::AuthSummary {
            zos_user: Some("alice".to_string()),
            ..aura_auth::AuthSummary::default()
        },
    )
}

#[tokio::test]
async fn gate_classifies_before_the_turn_and_records_its_default_route() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = signed_in_on_gate(&fixture);

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
