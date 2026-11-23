//! Where a comparison goes among the agents, and the name it gives the
//! agent that asked for it.

use super::*;

#[tokio::test]
async fn a_comparison_asked_from_another_goes_beside_it() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    state.compare = compare::CompareState::new(fixture._dirs[0].path().join("compare"), 1);

    let args = "--budget 0.001 write the answer";
    run_comparison(&mut state, &links, &mut commands, args).await;
    let first = subagents::lock(&state.hub.registry).compares()[0].clone();
    state.active = first.agent_id();
    run_comparison(&mut state, &links, &mut commands, args).await;

    let registry = subagents::lock(&state.hub.registry);
    let compares = registry.compares();
    assert_eq!(compares.len(), 2);
    assert_eq!(compares[1].parent, first.parent);
}

#[tokio::test]
async fn an_untitled_agent_that_runs_a_benchmark_is_named_after_it() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    state.compare = compare::CompareState::new(fixture._dirs[0].path().join("compare"), 1);
    let user = state.active;
    assert!(subagents::lock(&state.hub.registry)
        .find(user)
        .is_some_and(RegisteredAgent::needs_title));

    run_comparison(
        &mut state,
        &links,
        &mut commands,
        "--budget 0.001 write the answer",
    )
    .await;

    let registry = subagents::lock(&state.hub.registry);
    let agent = registry.find(user).unwrap();
    assert_eq!(agent.display_name(), "write the answer");
    assert!(!agent.needs_title());
}
