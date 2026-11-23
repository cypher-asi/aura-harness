//! A Gate run that fails its checks carrying on on a stronger model, and
//! what it records of the move.

use super::*;

#[tokio::test]
async fn a_gate_run_that_fails_its_checks_carries_on_on_a_stronger_model() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();

    let payload = run_comparison(
        &mut state,
        &links,
        &mut commands,
        "fix it (easy): run failing check --repeats 1 --against opus",
    )
    .await;

    assert_eq!(payload.status, CompareStatus::Done, "{payload:#?}");
    let routes = |name: &str| -> Vec<Route> {
        entries(&state, payload.run(name).unwrap().agent_id)
            .iter()
            .filter_map(|e| e.tx.prompt_payload())
            .map(|p| p.route)
            .collect()
    };
    let gate = payload.run("a-1").unwrap();
    assert_eq!(gate.error, None);
    assert_eq!(
        routes("a-1"),
        [
            Route::turn(HAIKU, RouteReason::Compare).at_effort("medium"),
            Route::turn(SONNET, RouteReason::Escalate).at_effort("medium"),
        ]
    );
    assert_eq!(gate.model, SONNET, "the run ends on the model it moved to");
    assert_turn_routing_recorded(gate);
    assert_eq!(
        signals_of(&state, gate.agent_id)[0],
        OutcomeSignal::ChecksFailed,
        "the failed attempt is recorded on its own"
    );
    assert_eq!(routes("b-1").len(), 1, "a run on one model stays on it");
}

/// Checks what `gate` recorded of its turn: how it was routed, the model it
/// started on, and the effort it ran at.
fn assert_turn_routing_recorded(gate: &CompareRun) {
    assert_eq!(
        (
            gate.turns[0].category,
            gate.turns[0].led,
            gate.turns[0].escalations
        ),
        (Some(PromptCategory::Debugging), Some(false), 1),
        "the run records how its turn was routed"
    );
    assert_eq!(
        gate.turns[0].first_model.as_deref(),
        Some(HAIKU),
        "and the model it started on"
    );
    assert_eq!(
        (
            gate.turns[0].effort.as_deref(),
            gate.turns[0].first_effort.as_deref()
        ),
        (Some("medium"), None),
        "and the effort it ran at, which a move to a stronger model keeps"
    );
}
