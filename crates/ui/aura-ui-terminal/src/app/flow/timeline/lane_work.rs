//! What one lane did, from its nodes: its turns as bars and its facts.

use super::{Mark, MarkKind, RowFacts, TurnSpan};
use crate::events::{FlowKind, FlowNode, TraceNodeStatus};

/// What a lane did, from its nodes.
#[derive(Default)]
pub(super) struct LaneWork {
    pub(super) spans: Vec<TurnSpan>,
    pub(super) facts: RowFacts,
    pub(super) first: Option<u64>,
    pub(super) last: u64,
}

/// Put `node` on the bar of `span`.
fn add(span: &mut TurnSpan, node: &FlowNode) {
    span.end_ms = span.end_ms.max(node.at_ms);
    match node.kind {
        FlowKind::Gate => span.marks.push(Mark {
            at_ms: node.at_ms,
            kind: MarkKind::Gate,
        }),
        FlowKind::Judge => span.marks.push(Mark {
            at_ms: node.at_ms,
            kind: MarkKind::Judge,
        }),
        _ => {}
    }
    if node.status == TraceNodeStatus::Error {
        span.marks.push(Mark {
            at_ms: node.at_ms,
            kind: MarkKind::Error,
        });
        if node.kind.is_model_call() {
            span.status = TraceNodeStatus::Error;
        }
    }
    if node.kind.is_model_call() {
        span.calls += 1;
    }
}

/// What `lane_work` keeps while it walks a lane's nodes.
struct Walk<'a> {
    nodes: &'a [FlowNode],
    work: LaneWork,
    // Gate classifies a prompt before the prompt is recorded.
    routing: Vec<usize>,
    cost: Option<f64>,
}

pub(super) fn lane_work(list: &[usize], nodes: &[FlowNode]) -> LaneWork {
    let mut walk = Walk {
        nodes,
        work: LaneWork::default(),
        routing: Vec::new(),
        cost: None,
    };
    for &at in list {
        walk.step(at);
    }
    walk.finish()
}

impl Walk<'_> {
    fn step(&mut self, at: usize) {
        let nodes = self.nodes;
        let node = &nodes[at];
        self.tally(node);
        if node.kind == FlowKind::Gate {
            self.routing.push(at);
            return;
        }
        let current = match node.kind {
            FlowKind::Prompt => None,
            _ => self.work.spans.last_mut(),
        };
        if let Some(span) = current {
            for gate in self.routing.drain(..) {
                add(span, &nodes[gate]);
            }
            add(span, node);
            return;
        }
        let span = self.open_span(at, node);
        self.work.spans.push(span);
    }

    /// Count `node` into the lane's time, errors and cost.
    fn tally(&mut self, node: &FlowNode) {
        let work = &mut self.work;
        work.first = Some(work.first.map_or(node.at_ms, |first| first.min(node.at_ms)));
        work.last = work.last.max(node.at_ms);
        if node.status == TraceNodeStatus::Error {
            work.facts.errors += 1;
        }
        if let Some(spent) = node.usage.and_then(|usage| usage.cost_usd) {
            self.cost = Some(self.cost.unwrap_or(0.0) + spent);
        }
    }

    /// A new turn starting at `node`, with the routing that came before it.
    fn open_span(&mut self, at: usize, node: &FlowNode) -> TurnSpan {
        let nodes = self.nodes;
        let start = self
            .routing
            .first()
            .map_or(node.at_ms, |&gate| nodes[gate].at_ms.min(node.at_ms));
        let status = match node.kind {
            FlowKind::Prompt => node.status,
            _ => TraceNodeStatus::Ok,
        };
        let mut span = TurnSpan {
            start_ms: start,
            end_ms: node.at_ms,
            head: at,
            status,
            marks: Vec::new(),
            calls: 0,
        };
        for gate in self.routing.drain(..) {
            add(&mut span, &nodes[gate]);
        }
        add(&mut span, node);
        span
    }

    /// Put routing no turn followed on the last turn, and sum up.
    fn finish(mut self) -> LaneWork {
        let nodes = self.nodes;
        let mut work = self.work;
        if let Some(&head) = self.routing.first() {
            let mut span = work.spans.pop().unwrap_or_else(|| TurnSpan {
                start_ms: nodes[head].at_ms,
                end_ms: nodes[head].at_ms,
                head,
                status: TraceNodeStatus::Ok,
                marks: Vec::new(),
                calls: 0,
            });
            for gate in self.routing.drain(..) {
                add(&mut span, &nodes[gate]);
            }
            work.spans.push(span);
        }
        work.facts.turns = work.spans.len();
        work.facts.calls = work.spans.iter().map(|span| span.calls).sum();
        work.facts.cost_usd = self.cost;
        work.facts.wall_ms = work.first.map(|first| work.last - first);
        work
    }
}
