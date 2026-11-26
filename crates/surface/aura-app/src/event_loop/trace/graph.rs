//! Live run graphs: their recorders, their terminal views and how changed
//! nodes reach the TRACE tab and the graph view.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use aura_agent_trace::graph::{GraphNode, NodeStatus};
use aura_agent_trace::{RunGraph, RunMeta, RunRecorder, RunSource};
use aura_core_types::trace::StepAction;
use aura_core_types::AgentId;
use aura_ui::events::{TraceNode, TraceNodeStatus};
use aura_ui::UiCommand;
use serde_json::Value;
use tokio::sync::mpsc;

use super::super::LoopState;
use super::lock;

pub(crate) fn status(s: NodeStatus) -> TraceNodeStatus {
    match s {
        NodeStatus::Pending => TraceNodeStatus::Pending,
        NodeStatus::Ok => TraceNodeStatus::Ok,
        NodeStatus::Warn => TraceNodeStatus::Warn,
        NodeStatus::Error => TraceNodeStatus::Error,
    }
}

pub(crate) fn enum_name<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

pub(crate) fn meta_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Terminal view of one graph node.
pub(crate) fn view(graph: &RunGraph, node: &GraphNode) -> TraceNode {
    TraceNode {
        id: node.id.clone(),
        kind: enum_name(&node.kind),
        parent: node.parent.clone(),
        label: node.label.clone(),
        detail: node.detail.clone(),
        status: status(node.status),
        seq: node.seq,
        meta: node
            .meta
            .iter()
            .map(|(k, v)| (k.clone(), meta_value(v)))
            .collect(),
        links: graph
            .links(&node.id)
            .into_iter()
            .filter(|e| e.from == node.id)
            .map(|e| (enum_name(&e.kind), e.to.clone()))
            .collect(),
    }
}

pub(crate) fn full_view(graph: &RunGraph) -> Vec<TraceNode> {
    graph.nodes.iter().map(|n| view(graph, n)).collect()
}

/// Recorder for `session`'s conversation, started on first use. `None`
/// when tracing is off or the bundle can't be created.
pub(crate) fn recorder(
    state: &LoopState<'_>,
    session: &super::super::agents::AgentSession,
    model: &str,
) -> Option<Arc<RunRecorder>> {
    let mut meta = RunMeta::new(model, RunSource::Tui);
    meta.workspace = session.kernel.agent_workspace().display().to_string();
    meta.mode = session.definition.clone().unwrap_or_default();
    state.hub.traces.start(session.agent_id(), meta)
}

/// Keep `graph` in step with the recorder and stream changed nodes to
/// `agent`'s TRACE tab and to the graph view, which ignores them unless
/// it shows this run.
pub(crate) fn subscribe_graph(
    recorder: &RunRecorder,
    agent: AgentId,
    graph: Arc<Mutex<RunGraph>>,
    commands: mpsc::Sender<UiCommand>,
) {
    let upsert = graph_sender(recorder.run_id(), agent, commands);
    let on_step = Arc::clone(&upsert);
    let step_graph = Arc::clone(&graph);
    recorder.subscribe(move |step| {
        let mut g = lock(&step_graph);
        let before = g.nodes.len();
        g.apply(step);
        let ids: Vec<String> = g
            .changed_since(before, step.seq)
            .into_iter()
            .map(|n| n.id.clone())
            .collect();
        on_step(&g, &ids);
    });
    recorder.subscribe_records(move |row| {
        let mut g = lock(&graph);
        if let Some(id) = g.apply_record(row) {
            upsert(&g, &[id]);
        }
    });
}

/// Link `child`'s run to the call that spawned it in the parent's live
/// `graph`, once its first prompt has given it a title.
pub(crate) fn link_to_parent(
    child: &Arc<RunRecorder>,
    parent: (AgentId, String),
    graph: Arc<Mutex<RunGraph>>,
    commands: mpsc::Sender<UiCommand>,
) {
    let upsert = graph_sender(parent.1, parent.0, commands);
    let child_rec = Arc::downgrade(child);
    let linked = AtomicBool::new(false);
    child.subscribe(move |step| {
        if !matches!(step.action, StepAction::UserPrompt { .. }) || linked.swap(true, Relaxed) {
            return;
        }
        let Some(child) = child_rec.upgrade() else {
            return;
        };
        let mut g = lock(&graph);
        let ids = g.link_child_runs(&[child.meta()]);
        upsert(&g, &ids);
    });
}

/// Sends the nodes of run `run_id`'s graph with the given ids to
/// `agent`'s TRACE tab and to the graph view.
pub(crate) type GraphSender = Arc<dyn Fn(&RunGraph, &[String]) + Send + Sync>;

pub(crate) fn graph_sender(
    run_id: String,
    agent: AgentId,
    commands: mpsc::Sender<UiCommand>,
) -> GraphSender {
    let agent_id = agent.to_hex();
    Arc::new(move |g, ids| {
        let nodes: Vec<TraceNode> = ids
            .iter()
            .filter_map(|id| g.node(id))
            .map(|n| view(g, n))
            .collect();
        if nodes.is_empty() {
            return;
        }
        let _ = commands.try_send(UiCommand::ForAgent {
            agent_id: agent_id.clone(),
            command: Box::new(UiCommand::GraphUpsert {
                run_id: run_id.clone(),
                nodes,
            }),
        });
    })
}
