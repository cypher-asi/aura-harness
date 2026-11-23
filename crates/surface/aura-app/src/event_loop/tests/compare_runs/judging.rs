//! The judge's verdicts on a comparison's pairs.

use super::*;

#[tokio::test]
async fn the_judge_compares_the_pairs_at_once() {
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
        "write the answer, judged together --repeats 2",
    )
    .await;

    assert_eq!(payload.status, CompareStatus::Done, "{payload:#?}");
    let pairs: Vec<(&str, &str, bool)> = (payload.judgments.iter())
        .map(|j| (j.a_run.as_str(), j.b_run.as_str(), j.parsed))
        .collect();
    assert_eq!(
        pairs,
        [("a-1", "b-1", true), ("a-2", "b-2", true)],
        "both pairs are judged together, and kept in order: {:#?}",
        payload.judgments
    );
}
