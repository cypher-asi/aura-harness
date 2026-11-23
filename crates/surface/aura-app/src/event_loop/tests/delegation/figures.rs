//! `/gate`'s delegation figures, for a lead whose subtask was retried.

use super::*;

#[tokio::test]
async fn gate_shows_the_leads_subtask_and_its_retry_in_the_delegation_figures() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    retried_once(&fixture, &mut state).await;

    let delegation = gate::view::delegation_of(&fixture.hub, first);
    assert_eq!(delegation.turns.len(), 1);
    let overall = delegation.overall;
    assert_eq!((overall.subtasks, overall.escalations), (1, 1));
    assert!(overall.subtask_calls >= 4, "{overall:?}");
    assert_eq!(
        overall.cost_share(),
        Some(1.0),
        "the lead itself made no calls"
    );
    assert_eq!(overall.escalation_rate(), Some(1.0));
    drain(&mut commands);
    handle_ui_event(&mut state, UiEvent::ShowGate, &links).await;
    let text = system_texts(&drain(&mut commands));
    assert!(
        text.contains(
            "Delegation in this agent's 1 turn: 1.0 subtasks per turn, escalation rate 100% (1 of 1)"
        ),
        "{text}"
    );
}
