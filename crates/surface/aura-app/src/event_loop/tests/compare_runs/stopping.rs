//! What stops a comparison: its budget, or Aura quitting while it runs.

use super::*;
use aura_ui::events::BenchStatus;

#[tokio::test]
async fn the_budget_stops_new_runs_once_spent() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    state.compare = compare::CompareState::new(fixture._dirs[0].path().join("compare"), 1);

    let payload = run_comparison(
        &mut state,
        &links,
        &mut commands,
        "--budget 0.001 write the answer",
    )
    .await;

    let names: Vec<&str> = payload.runs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["a-1"], "one run spent the budget");
    assert!(payload.runs[0].cost_usd >= 0.001);
    assert!(
        payload
            .notes
            .iter()
            .any(|n| n.contains("5 runs did not start")),
        "{:?}",
        payload.notes
    );
    assert!(payload.judgments.is_empty(), "nothing to pair it with");
    let registry = subagents::lock(&state.hub.registry);
    let runs: Vec<&str> = registry
        .agents()
        .iter()
        .filter(|a| registry.under_compare(a.agent_id()))
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(runs, ["a-1"], "the runs that did not start are dropped");
}

#[tokio::test]
async fn a_comparison_recorded_running_that_nothing_runs_is_stopped() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    state.compare = compare::CompareState::new(fixture._dirs[0].path().join("compare"), 1);
    let mut payload = run_comparison(
        &mut state,
        &links,
        &mut commands,
        "--budget 0.001 write the answer",
    )
    .await;

    // As Aura leaves it when it quits mid-run: the last record says running.
    let host = subagents::lock(&state.hub.registry).compares()[0].clone();
    payload.status = CompareStatus::Running;
    let kernel = compare::kernel_of(&mut state, host.agent_id()).unwrap();
    let ids = (host.agent_id(), host.name.as_str());
    compare::run::save(&state.hub, &kernel, state.commands, ids, &payload).await;
    drain(&mut commands);

    bench(&mut state, &links, BenchRequest::List).await;
    let listed = bench_list(&mut commands).unwrap();
    assert_eq!(listed[0].status, BenchStatus::Cancelled);
    assert!(
        listed[0]
            .notes
            .iter()
            .any(|n| n.contains("stopped before it ended")),
        "{:?}",
        listed[0].notes
    );

    compare(&mut state, &links, "dismiss").await;
    assert_eq!(latest(&state).status, CompareStatus::Dismissed);
}
