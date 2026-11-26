//! What Gate learns, its criteria and the gate table.

use super::*;

#[tokio::test]
async fn gate_explores_the_next_cheaper_model_and_records_its_propensity() {
    let gate = GateLive::with_rng(
        aura_config::GateConfig {
            explore_rate: 1.0,
            ..aura_config::GateConfig::defaults()
        },
        aura_gate::route::SplitMix64::seeded(3),
    );
    let (fixture, mut commands) = fixture_with_gate(gate);
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);

    turn(&mut state, &mut done, &links, first, "what does gate do?").await;

    let (_, _, prompt) = prompts_of(&state, first).remove(0);
    assert_eq!(
        prompt.route,
        Route {
            explored: true,
            propensity: 1.0,
            ..Route::turn("aura-gpt-5-4", RouteReason::Explore).at_effort("medium")
        },
        "the default for questions is sonnet; the next cheaper is gpt-5.4"
    );
    let (_, outcome) = outcomes_of(&state, first).remove(0);
    assert_eq!(outcome.model, "aura-gpt-5-4");
    assert_eq!(
        route_lines(&mut commands),
        ["gate → aura-gpt-5-4 (cloud) at medium effort · question, explore"]
    );
}

/// A later signal about `agent`'s last turn, as the judge records one.
async fn rate(state: &LoopState<'_>, agent: AgentId, signal: OutcomeSignal) {
    let kernel = state.kernel_for(agent).unwrap();
    let last = aura_gate::live::last_turn(kernel.store().as_ref(), agent).unwrap();
    assert!(aura_gate::live::record(&kernel, agent, &last.outcome.with_signal(signal)).await);
}

/// Why each of `agent`'s prompts was routed as it was, oldest first.
fn route_reasons(state: &LoopState<'_>, agent: AgentId) -> Vec<RouteReason> {
    prompts_of(state, agent)
        .into_iter()
        .map(|(_, _, p)| p.route.reason)
        .collect()
}

/// Gate's row of Sonnet answering questions in `fixture`.
fn sonnet_question_row(fixture: &Fixture) -> aura_gate::route::stats::Row {
    fixture
        .hub
        .gate
        .stats()
        .row(
            RouteLevel::Turn,
            PromptCategory::Question,
            "aura-claude-sonnet-4-6",
        )
        .copied()
        .unwrap()
}

#[tokio::test]
async fn the_next_turn_uses_what_the_last_outcome_taught_gate() {
    let gate = GateLive::new(aura_config::GateConfig {
        min_tries: 1,
        ..aura_config::GateConfig::defaults().without_exploring()
    });
    let (fixture, _commands) = fixture_with_gate(gate);
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    let reasons = |state: &LoopState<'_>| route_reasons(state, first);

    turn(&mut state, &mut done, &links, first, "what is a monad?").await;
    rate(&state, first, OutcomeSignal::JudgedPass).await;
    turn(&mut state, &mut done, &links, first, "what is a functor?").await;
    assert_eq!(reasons(&state), [RouteReason::Default, RouteReason::Proven]);

    rate(&state, first, OutcomeSignal::JudgedFail).await;
    let row = sonnet_question_row(&fixture);
    assert_eq!(
        (row.tries, row.successes),
        (2, 1),
        "each prompt counts once"
    );
    turn(
        &mut state,
        &mut done,
        &links,
        first,
        "what is an applicative?",
    )
    .await;
    assert_eq!(
        reasons(&state),
        [
            RouteReason::Default,
            RouteReason::Proven,
            RouteReason::Default
        ],
        "one of two is below the target, so sonnet drops back to the default"
    );
}

#[tokio::test]
async fn a_criteria_writer_that_fails_says_so_and_the_turn_goes_on() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);

    turn(
        &mut state,
        &mut done,
        &links,
        first,
        "fix the parser (criteria writer down)",
    )
    .await;

    let text = system_texts(&drain(&mut commands));
    assert!(
        text.contains(
            "gate: aura-claude-sonnet-4-6 (cloud) wrote no acceptance criteria: the call failed ("
        ),
        "{text}"
    );
    assert!(text.contains("not supported for this model"), "{text}");
    assert!(
        text.contains("gate → "),
        "the turn is routed all the same:\n{text}"
    );
    assert_eq!(outcomes_of(&state, first).len(), 1, "and runs to its end");
}

/// The difficulty the prompt of a turn answering `text` is recorded with,
/// under Gate with acceptance criteria `criteria`.
async fn recorded_difficulty(
    criteria: aura_config::ReviewMode,
    text: &str,
) -> Option<aura_core_types::Difficulty> {
    let (fixture, _commands) = fixture_with_gate(GateLive::new(aura_config::GateConfig {
        explore_rate: 0.0,
        criteria,
        ..aura_config::GateConfig::defaults()
    }));
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    turn(&mut state, &mut done, &links, first, text).await;
    prompts_of(&state, first)[0].2.difficulty
}

#[tokio::test]
async fn exacting_criteria_rate_the_work_hard_unless_criteria_are_off() {
    use aura_config::ReviewMode;
    let exacting = "fix the parser (exacting criteria)";
    assert_eq!(
        recorded_difficulty(ReviewMode::Log, exacting).await,
        Some(Difficulty::Hard),
        "logged criteria route the work as acting ones do"
    );
    assert_eq!(
        recorded_difficulty(ReviewMode::On, exacting).await,
        Some(Difficulty::Hard)
    );
    assert_eq!(
        recorded_difficulty(ReviewMode::Off, exacting).await,
        Some(Difficulty::Medium),
        "no criteria are written"
    );
    assert_eq!(
        recorded_difficulty(ReviewMode::Log, "fix the parser (plain criteria)").await,
        Some(Difficulty::Medium),
        "criteria that are not exacting leave the classifier's difficulty"
    );
}

/// What the model doing a turn answering `text` was sent as its request,
/// under Gate with acceptance criteria `criteria`.
async fn request_sent(criteria: aura_config::ReviewMode, text: &str) -> String {
    let (fixture, _commands) = fixture_with_gate(GateLive::new(aura_config::GateConfig {
        explore_rate: 0.0,
        criteria,
        ..aura_config::GateConfig::defaults()
    }));
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    turn(&mut state, &mut done, &links, first, text).await;
    let sent = user_texts(&state, first).remove(0);
    assert_eq!(
        prompts_of(&state, first)[0].2.text,
        text,
        "the record keeps the prompt as typed"
    );
    sent
}

#[tokio::test]
async fn the_worker_is_shown_the_criteria_unless_criteria_are_off() {
    use aura_config::ReviewMode;
    let text = "fix the parser (plain criteria)";
    let note = "the harness wrote down what this work will be checked against";
    let criterion = "prints the parse tree";
    for mode in [ReviewMode::Log, ReviewMode::On] {
        let sent = request_sent(mode, text).await;
        assert!(sent.starts_with(text), "{mode}: {sent}");
        assert!(
            sent.contains(note) && sent.contains(criterion),
            "{mode}: {sent}"
        );
    }
    assert_eq!(request_sent(ReviewMode::Off, text).await, text);
    let unlisted = "fix the parser";
    assert_eq!(
        request_sent(ReviewMode::Log, unlisted).await,
        unlisted,
        "nothing is added when the writer listed none"
    );
}

#[tokio::test]
async fn slash_gate_shows_the_table() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    learn_questions(&fixture.hub.gate, "aura-claude-haiku-4-5", 10);

    handle_ui_event(&mut state, UiEvent::ShowGate, &links).await;

    let text = system_texts(&drain(&mut commands));
    assert!(text.contains("Turns"), "{text}");
    assert!(
        text.contains("question            → aura-claude-sonnet-4-6 (cloud)"),
        "{text}"
    );
    assert!(text.contains("→ aura-claude-haiku-4-5 (cloud)"), "{text}");
    assert!(text.contains("pick for easy work (proven)"), "{text}");
}
