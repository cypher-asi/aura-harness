//! The share of the active agent's cost and model calls spent in
//! subtasks, overall and for its last turns.

use super::{render, section};
use crate::record_loader::format_local_time;
use aura_gate::lead::delegation::{self, Call, Delegation, Subtask};
use aura_gate::route::stats::GateStats;

/// A lead's two turns, one model call in each, and the first turn's two
/// subtasks: one of them the retry of the other on a stronger model.
fn delegated() -> Delegation {
    let calls = [1_100, 2_100].map(|ts_ms| Call {
        ts_ms,
        cost_usd: 0.10,
    });
    let subtask = |ts_ms, escalated, cost_usd| Subtask {
        ts_ms,
        escalated,
        calls: 3,
        cost_usd,
    };
    let subtasks = [subtask(1_200, false, 0.20), subtask(1_300, true, 0.40)];
    delegation::summarize(&[1_000, 2_000], &calls, &subtasks)
}

#[test]
fn the_view_shows_the_share_of_work_in_subtasks() {
    let text = render(&GateStats::default(), &[], &delegated());
    assert!(
        text.contains(
            "Delegation in this agent's 2 turns: 0.5 subtasks per turn, escalation rate \
             100% (1 of 1)\n  75% of cost in subtasks ($0.600 of $0.800), 75% (6 of 8) of \
             model calls"
        ),
        "{text}"
    );
    assert!(
        text.contains("1 subtask · 86% of cost in subtasks"),
        "{text}"
    );
    assert!(
        text.contains("0 subtasks · 0% of cost in subtasks"),
        "{text}"
    );
}

#[test]
fn the_delegation_figures_are_pinned() {
    let expected = format!(
        "Delegation in this agent's 2 turns: 0.5 subtasks per turn, escalation rate 100% (1 of 1)\n  \
         75% of cost in subtasks ($0.600 of $0.800), 75% (6 of 8) of model calls\n  \
         {}  1 subtask · 86% of cost in subtasks ($0.600 of $0.700), 86% (6 of 7) of model calls\n  \
         {}  0 subtasks · 0% of cost in subtasks (free of $0.100), 0% (0 of 1) of model calls",
        format_local_time(1_000),
        format_local_time(2_000),
    );
    let text = render(&GateStats::default(), &[], &delegated());
    assert_eq!(section(&text, "Delegation"), expected);
}

#[test]
fn turns_with_no_calls_have_no_shares_and_only_the_last_five_are_listed() {
    let starts: Vec<u64> = (1..=7).map(|n| n * 1_000).collect();
    let mut expected = "Delegation in this agent's 7 turns: 0.0 subtasks per turn, \
                        escalation rate -\n  no cost, - of model calls"
        .to_string();
    for ts_ms in &starts[2..] {
        expected.push_str(&format!(
            "\n  {}  0 subtasks · no cost, - of model calls",
            format_local_time(*ts_ms)
        ));
    }
    let delegation = delegation::summarize(&starts, &[], &[]);
    let text = render(&GateStats::default(), &[], &delegation);
    assert_eq!(section(&text, "Delegation"), expected);
}
