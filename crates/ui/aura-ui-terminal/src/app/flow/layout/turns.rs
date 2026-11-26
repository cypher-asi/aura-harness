//! Grouping a flow's nodes into turns, before they are placed.

use super::{CardKind, NONE};
use crate::events::{FlowKind, FlowNode, FlowUsage};
use std::collections::HashMap;

/// A subcard before it is placed.
pub(super) struct SubDraft {
    pub(super) kind: CardKind,
    pub(super) head: usize,
    pub(super) rows: Vec<usize>,
}

/// A turn before it is placed.
pub(super) struct TurnDraft {
    pub(super) head: usize,
    pub(super) rows: Vec<usize>,
    pub(super) subs: Vec<SubDraft>,
    pub(super) lane: usize,
    /// When it started, then its lane and record order
    pub(super) key: (u64, usize, u64),
}

/// What the model calls of turn `draft` used together, and how long the
/// turn has taken: from its prompt to the last of its steps. `None`
/// before its first call is done.
pub(super) fn turn_stats(draft: &TurnDraft, nodes: &[FlowNode]) -> Option<FlowUsage> {
    let mut total: Option<FlowUsage> = None;
    let mut last = nodes[draft.head].at_ms;
    for sub in &draft.subs {
        for &node in std::iter::once(&sub.head).chain(&sub.rows) {
            last = last.max(nodes[node].at_ms);
        }
        if let Some(usage) = nodes[sub.head].usage {
            total = Some(total.map_or(usage, |total| total.plus(usage)));
        }
    }
    for &node in &draft.rows {
        last = last.max(nodes[node].at_ms);
    }
    let wall = last.saturating_sub(nodes[draft.head].at_ms);
    total.map(|total| FlowUsage {
        duration_ms: Some(total.duration_ms.unwrap_or(0).max(wall)),
        ..total
    })
}

/// Whether `kind` is a model call the harness made around the agent's
/// work: Gate's, or the chat's title.
const fn harness_call(kind: FlowKind) -> bool {
    matches!(kind, FlowKind::Gate | FlowKind::Judge | FlowKind::Title)
}

/// A model call's subcard, before its rows.
const fn call(head: usize) -> SubDraft {
    SubDraft {
        kind: CardKind::Call,
        head,
        rows: Vec::new(),
    }
}

/// The row of the `task` call each result came back to; [`NONE`] for
/// other nodes.
pub(super) fn answered_rows(nodes: &[FlowNode], index: &HashMap<String, usize>) -> Vec<usize> {
    let mut answered = vec![NONE; nodes.len()];
    for (at, node) in nodes.iter().enumerate() {
        if node.kind != FlowKind::Tool {
            continue;
        }
        for (kind, other) in &node.links {
            if let Some(&result) = index.get(other).filter(|_| kind == "result") {
                if nodes[result].kind == FlowKind::Result {
                    answered[result] = at;
                }
            }
        }
    }
    answered
}

/// Each lane's nodes, in the order of `nodes`.
fn lane_nodes(lane_index: &HashMap<String, usize>, nodes: &[FlowNode]) -> Vec<Vec<usize>> {
    let mut per_lane: Vec<Vec<usize>> = vec![Vec::new(); lane_index.len()];
    for (at, node) in nodes.iter().enumerate() {
        if let Some(&lane) = lane_index.get(&node.agent_id) {
            per_lane[lane].push(at);
        }
    }
    per_lane
}

/// The turns, sorted by when they started, and each node's lane clock.
///
/// Within a lane the record's order holds; across lanes, time does. A
/// lane's clock never runs backwards, so the two cannot disagree.
pub(super) fn group_turns(
    lane_index: &HashMap<String, usize>,
    nodes: &[FlowNode],
    answered: &[usize],
) -> (Vec<TurnDraft>, Vec<u64>) {
    let mut per_lane = lane_nodes(lane_index, nodes);
    let mut drafts: Vec<TurnDraft> = Vec::new();
    let mut clock_of = vec![0u64; nodes.len()];
    for (lane, list) in per_lane.iter_mut().enumerate() {
        group_lane(lane, list, nodes, answered, &mut drafts, &mut clock_of);
    }
    drafts.sort_by_key(|draft| (draft.key, draft.head));
    (drafts, clock_of)
}

/// Group lane `lane`'s nodes `list` into turns, appended to `drafts`.
fn group_lane(
    lane: usize,
    list: &mut [usize],
    nodes: &[FlowNode],
    answered: &[usize],
    drafts: &mut Vec<TurnDraft>,
    clock_of: &mut [u64],
) {
    list.sort_by_key(|&at| (nodes[at].seq, at));
    let first = drafts.len();
    let mut clock = 0;
    // Gate classifies a prompt before the prompt is recorded; its call
    // is the first step of the turn the prompt starts.
    let mut routing: Vec<usize> = Vec::new();
    for &at in list.iter() {
        let node = &nodes[at];
        clock = clock.max(node.at_ms);
        clock_of[at] = clock;
        if answered[at] != NONE {
            continue;
        }
        if node.kind == FlowKind::Gate {
            routing.push(at);
            continue;
        }
        let current = match node.kind {
            FlowKind::Prompt => None,
            _ => drafts.get_mut(first..).and_then(<[TurnDraft]>::last_mut),
        };
        let Some(turn) = current else {
            drafts.push(TurnDraft {
                head: at,
                rows: Vec::new(),
                subs: routing.drain(..).map(call).collect(),
                lane,
                key: (clock, lane, node.seq),
            });
            continue;
        };
        add_step(turn, at, nodes);
    }
    flush_routing(lane, first, routing, nodes, clock_of, drafts);
}

/// Put node `at` in turn `turn`: as a row, or a subcard, or a row of one.
fn add_step(turn: &mut TurnDraft, at: usize, nodes: &[FlowNode]) {
    match nodes[at].kind {
        FlowKind::Reply => turn.rows.push(at),
        kind if kind.is_model_call() => turn.subs.push(call(at)),
        FlowKind::Message | FlowKind::Result => turn.subs.push(SubDraft {
            kind: CardKind::Note,
            head: at,
            rows: Vec::new(),
        }),
        // What the agent did goes under the agent's own last step,
        // never under a call the harness made around its work.
        _ => match turn
            .subs
            .iter_mut()
            .rev()
            .find(|sub| !harness_call(nodes[sub.head].kind))
        {
            Some(sub) => sub.rows.push(at),
            None => turn.rows.push(at),
        },
    }
}

/// A prompt classified and not yet recorded: until it is, its call
/// waits in the turn before it.
fn flush_routing(
    lane: usize,
    first: usize,
    mut routing: Vec<usize>,
    nodes: &[FlowNode],
    clock_of: &[u64],
    drafts: &mut Vec<TurnDraft>,
) {
    if routing.is_empty() {
        return;
    }
    match drafts.get_mut(first..).and_then(<[TurnDraft]>::last_mut) {
        Some(turn) => turn.subs.extend(routing.drain(..).map(call)),
        None => {
            let head = routing.remove(0);
            drafts.push(TurnDraft {
                head,
                rows: Vec::new(),
                subs: routing.drain(..).map(call).collect(),
                lane,
                key: (clock_of[head], lane, nodes[head].seq),
            });
        }
    }
}
