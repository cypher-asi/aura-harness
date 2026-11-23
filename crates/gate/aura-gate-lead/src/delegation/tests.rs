//! Which turn of the lead each model call and subtask belongs to, and the
//! shares that come out of it.

use super::{summarize, Call, Subtask};

fn call(ts_ms: u64, cost_usd: f64) -> Call {
    Call { ts_ms, cost_usd }
}

fn subtask(ts_ms: u64, escalated: bool, calls: u32, cost_usd: f64) -> Subtask {
    Subtask {
        ts_ms,
        escalated,
        calls,
        cost_usd,
    }
}

#[test]
fn calls_and_subtasks_belong_to_the_turn_they_happened_in() {
    let delegation = summarize(
        &[100, 200],
        &[call(110, 0.10), call(150, 0.10), call(210, 0.20)],
        &[
            subtask(120, false, 3, 0.30),
            subtask(130, false, 2, 0.10),
            subtask(140, true, 4, 0.40),
            subtask(220, false, 1, 0.05),
        ],
    );
    assert_eq!(delegation.turns.len(), 2);
    let first = delegation.turns[0].share;
    assert_eq!(first.lead_calls, 2);
    assert_eq!(first.subtasks, 2);
    assert_eq!(first.escalations, 1);
    assert_eq!(first.subtask_calls, 9);
    assert!((first.cost_share().unwrap() - 0.8 / 1.0).abs() < 1e-9);
    assert_eq!(first.escalation_rate(), Some(0.5));

    let overall = delegation.overall;
    assert_eq!(overall.lead_calls, 3);
    assert_eq!(overall.subtask_calls, 10);
    assert_eq!(overall.subtasks, 3);
    assert!((overall.call_share().unwrap() - 10.0 / 13.0).abs() < 1e-9);
    assert!((overall.cost_share().unwrap() - 0.85 / 1.25).abs() < 1e-9);
    assert_eq!(delegation.subtasks_per_turn(), Some(1.5));
}

#[test]
fn work_before_the_first_turn_counts_in_it_and_no_turns_say_nothing() {
    let delegation = summarize(&[100], &[call(90, 0.01)], &[subtask(50, false, 1, 0.02)]);
    assert_eq!(delegation.turns[0].share.lead_calls, 1);
    assert_eq!(delegation.turns[0].share.subtasks, 1);

    let none = summarize(&[], &[call(1, 1.0)], &[]);
    assert!(none.turns.is_empty());
    assert_eq!(none.subtasks_per_turn(), None);
    assert_eq!(none.overall.cost_share(), None);
    assert_eq!(none.overall.call_share(), None);
    assert_eq!(none.overall.escalation_rate(), None);
}

#[test]
fn turns_given_out_of_order_count_in_order() {
    let calls = [call(110, 0.10), call(210, 0.20), call(310, 0.30)];
    let subtasks = [subtask(120, false, 1, 0.05), subtask(320, true, 2, 0.10)];
    let delegation = summarize(&[300, 100, 200, 100], &calls, &subtasks);
    let starts: Vec<u64> = delegation.turns.iter().map(|turn| turn.ts_ms).collect();
    assert_eq!(starts, [100, 100, 200, 300]);
    let lead_calls: Vec<u32> = delegation
        .turns
        .iter()
        .map(|turn| turn.share.lead_calls)
        .collect();
    assert_eq!(lead_calls, [0, 1, 1, 1]);
    assert_eq!(delegation.turns[1].share.subtasks, 1);
    assert_eq!(delegation.turns[3].share.escalations, 1);
    assert_eq!(
        delegation,
        summarize(&[100, 100, 200, 300], &calls, &subtasks)
    );
}
