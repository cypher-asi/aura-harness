//! Flows loaded into the graph: a lead with its helpers, and a benchmark.

use crate::shell::tests::Stage;
use aura_ui::events::{FlowKind, FlowLane, FlowNode, FlowUsage, TraceNodeStatus, UiCommand};

fn node(agent: &str, seq: u64, kind: FlowKind, label: &str, detail: &str) -> FlowNode {
    FlowNode {
        id: format!("{agent}:{seq}"),
        agent_id: agent.into(),
        kind,
        label: label.into(),
        detail: detail.into(),
        status: TraceNodeStatus::Ok,
        seq,
        at_ms: 1_000 * seq + if agent == "a1" { 0 } else { 3_500 },
        ..FlowNode::default()
    }
}

fn linked(mut node: FlowNode, kind: &str, to: &str) -> FlowNode {
    node.links.push((kind.into(), to.into()));
    node
}

fn lane(id: &str, name: &str, parent: Option<&str>) -> FlowLane {
    FlowLane {
        agent_id: id.into(),
        name: name.into(),
        parent: parent.map(Into::into),
        ..FlowLane::default()
    }
}

fn used(
    mut node: FlowNode,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: Option<f64>,
    ms: u64,
) -> FlowNode {
    node.usage = Some(FlowUsage {
        input_tokens,
        output_tokens,
        cost_usd,
        duration_ms: Some(ms),
    });
    node
}

/// The lead's prompt, model call, tool call and the two tasks it gave.
fn lead_start() -> [FlowNode; 5] {
    [
        node("a1", 1, FlowKind::Prompt, "Summarise the crates", ""),
        used(
            node("a1", 2, FlowKind::Model, "qwen3-coder", "1.2k/300 · 4s"),
            48_200,
            300,
            Some(0.042),
            4_000,
        ),
        node("a1", 3, FlowKind::Tool, "list_files crates", "12ms"),
        linked(
            linked(
                node("a1", 4, FlowKind::Tool, "task coder · fix README", "8.1s"),
                "spawn",
                "c1:1",
            ),
            "result",
            "a1:7",
        ),
        linked(
            linked(
                node("a1", 5, FlowKind::Tool, "task tester · cargo test", "6.3s"),
                "spawn",
                "t1:1",
            ),
            "result",
            "a1:8",
        ),
    ]
}

/// What the coder and the tester did.
fn helpers() -> [FlowNode; 7] {
    [
        node("c1", 1, FlowKind::Prompt, "fix README", ""),
        used(
            node("c1", 2, FlowKind::Model, "qwen3-coder", "800/120 · 2s"),
            800,
            120,
            None,
            2_000,
        ),
        node("c1", 3, FlowKind::Tool, "edit_file README.md", "4ms"),
        linked(node("c1", 4, FlowKind::Reply, "done", ""), "return", "a1:7"),
        node("t1", 1, FlowKind::Prompt, "cargo test", ""),
        node("t1", 2, FlowKind::Tool, "run_command cargo test", "5.9s"),
        linked(
            node("t1", 3, FlowKind::Reply, "42 passed", ""),
            "return",
            "a1:8",
        ),
    ]
}

/// What a lead did, and the coder and tester it gave work to.
pub fn flow(h: &mut Stage) {
    let mut pending = node("a1", 9, FlowKind::Model, "qwen3-coder", "");
    pending.status = TraceNodeStatus::Pending;
    let mut nodes = Vec::new();
    nodes.extend(lead_start());
    nodes.extend(helpers());
    nodes.extend([
        node("a1", 7, FlowKind::Result, "coder #1 replied", ""),
        node("a1", 8, FlowKind::Result, "tester #1 replied", ""),
        pending,
    ]);
    h.apply([UiCommand::FlowLoad {
        agent_id: "a1".into(),
        lanes: vec![
            lane("a1", "Agent01", None),
            lane("c1", "coder #1", Some("a1")),
            lane("t1", "tester #1", Some("a1")),
        ],
        nodes,
    }]);
}

fn timed(agent: &str, seq: u64, at_ms: u64, kind: FlowKind, label: &str) -> FlowNode {
    FlowNode {
        id: format!("{agent}:{seq}"),
        agent_id: agent.into(),
        kind,
        label: label.into(),
        status: TraceNodeStatus::Ok,
        seq,
        at_ms,
        ..FlowNode::default()
    }
}

/// The lead's eight turns.
fn lead_turns(nodes: &mut Vec<FlowNode>) {
    for turn in 0..8 {
        let at = 1_000 * turn;
        nodes.push(timed("a1", 3 * turn + 1, at, FlowKind::Prompt, "next step"));
        nodes.push(timed(
            "a1",
            3 * turn + 2,
            at + 100,
            FlowKind::Model,
            "qwen3-coder",
        ));
        nodes.push(timed("a1", 3 * turn + 3, at + 900, FlowKind::Reply, "done"));
    }
}

/// The lane of one arm's run of a task.
fn arm_lane(id: &str, task: u64, arm: usize, letter: char, check: bool) -> FlowLane {
    FlowLane {
        bench: Some(aura_ui::events::FlowBench {
            group: format!("coding/task-{task}"),
            run: format!("{letter}-{}", task + 1),
            arm,
            arm_label: if arm == 0 { "gate" } else { "opus" }.into(),
            check_passed: Some(check),
            cost_usd: 0.12,
            wall_ms: 40_000,
            error: None,
        }),
        ..lane(id, &format!("{letter}-{}", task + 1), Some("h1"))
    }
}

/// The two turns of one arm's run of a task.
fn arm_turns(nodes: &mut Vec<FlowNode>, id: &str, task: u64, check: bool) {
    for turn in 0..2 {
        let at = 1_000 * task + 400 * turn;
        nodes.push(timed(id, 3 * turn + 1, at, FlowKind::Prompt, "fix it"));
        let mut model = timed(id, 3 * turn + 2, at + 50, FlowKind::Model, "opus");
        if !check {
            model.status = TraceNodeStatus::Error;
        }
        nodes.push(model);
        nodes.push(timed(id, 3 * turn + 3, at + 300, FlowKind::Reply, "fixed"));
    }
}

/// A lead that ran a benchmark of six tasks on two arms, and kept working.
pub(super) fn benchmark_flow(h: &mut Stage) {
    let mut lanes = vec![
        lane("a1", "Agent01", None),
        lane("h1", "Compare #1", Some("a1")),
    ];
    let mut nodes = Vec::new();
    lead_turns(&mut nodes);
    for task in 0..6_u64 {
        for arm in 0..2_usize {
            let id = format!("r{task}{arm}");
            let letter = if arm == 0 { 'a' } else { 'b' };
            let check = !(arm == 1 && task % 3 == 0);
            lanes.push(arm_lane(&id, task, arm, letter, check));
            arm_turns(&mut nodes, &id, task, check);
        }
    }
    h.apply([UiCommand::FlowLoad {
        agent_id: "a1".into(),
        lanes,
        nodes,
    }]);
}
