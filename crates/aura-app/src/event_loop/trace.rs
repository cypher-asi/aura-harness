//! Run traces for TUI conversations.
//!
//! Each agent's conversation is one run under `AURA_HOME/runs/`. The
//! recorder starts on the agent's first turn and is finished when the
//! session is reset or the TUI quits; a scorecard is written after
//! every turn so `aura runs` sees live data. A live [`RunGraph`] per run
//! feeds `/graph` and streams node updates while the graph is open.

use std::sync::{Arc, Mutex, PoisonError};

use aura_agent::{AgentError, AgentLoopResult};
use aura_agent_trace::bundle::{list_runs, RunBundle};
use aura_agent_trace::graph::{GraphNode, NodeStatus};
use aura_agent_trace::{report, RunGraph, RunMeta, RunRecorder, RunSource, RunTrace};
use aura_core_types::trace::{Reward, RewardKind, RewardTarget};
use aura_core_types::AgentId;
use aura_terminal::events::{TraceNode, TraceNodeStatus};
use aura_terminal::UiCommand;
use serde_json::Value;
use tokio::sync::mpsc;

use super::agents::AgentSession;
use super::handlers::show_system_message;
use super::LoopState;

/// Runs shown by `/runs`.
const RUNS_LISTED: usize = 15;

/// An open run: its recorder and live graph.
pub(crate) struct LiveRun {
    recorder: Arc<RunRecorder>,
    graph: Arc<Mutex<RunGraph>>,
}

fn status(s: NodeStatus) -> TraceNodeStatus {
    match s {
        NodeStatus::Pending => TraceNodeStatus::Pending,
        NodeStatus::Ok => TraceNodeStatus::Ok,
        NodeStatus::Warn => TraceNodeStatus::Warn,
        NodeStatus::Error => TraceNodeStatus::Error,
    }
}

fn enum_name<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn meta_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
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

fn full_view(graph: &RunGraph) -> Vec<TraceNode> {
    graph.nodes.iter().map(|n| view(graph, n)).collect()
}

/// Recorder for `session`'s conversation, started on first use. `None`
/// when tracing is off or the bundle can't be created.
pub(super) fn recorder(
    state: &mut LoopState<'_>,
    session: &AgentSession,
    model: &str,
) -> Option<Arc<RunRecorder>> {
    let agent_id = session.agent_id();
    if let Some(live) = state.traces.get(&agent_id) {
        return Some(Arc::clone(&live.recorder));
    }
    let mut meta = RunMeta::new(model, RunSource::Tui);
    meta.workspace = session.kernel.agent_workspace().display().to_string();
    meta.mode = session.definition.clone().unwrap_or_default();
    meta.task_id = Some(format!("tui:{}", agent_id.to_hex()));
    let recorder = RunRecorder::start_default(meta)?;
    let graph = Arc::new(Mutex::new(RunGraph::new(&recorder.meta())));
    subscribe_graph(&recorder, Arc::clone(&graph), state.commands.clone());
    state.record_router.attach(agent_id, &recorder);
    state.traces.insert(
        agent_id,
        LiveRun {
            recorder: Arc::clone(&recorder),
            graph,
        },
    );
    Some(recorder)
}

/// Keep `graph` in step with the recorder and stream changed nodes to
/// the terminal, which ignores them unless it shows this run.
fn subscribe_graph(
    recorder: &RunRecorder,
    graph: Arc<Mutex<RunGraph>>,
    commands: mpsc::Sender<UiCommand>,
) {
    let run_id = recorder.run_id();
    recorder.subscribe(move |step| {
        let mut g = graph.lock().unwrap_or_else(PoisonError::into_inner);
        let before = g.nodes.len();
        g.apply(step);
        let nodes: Vec<TraceNode> = g
            .changed_since(before, step.seq)
            .into_iter()
            .map(|n| view(&g, n))
            .collect();
        if !nodes.is_empty() {
            let _ = commands.try_send(UiCommand::GraphUpsert {
                run_id: run_id.clone(),
                nodes,
            });
        }
    });
}

/// Record how a turn ended and refresh the run's scorecard.
pub(super) fn end_turn(
    rec: &RunRecorder,
    result: &Result<AgentLoopResult, AgentError>,
    cancelled: bool,
) {
    if cancelled {
        rec.interrupted();
    }
    match result {
        Ok(result) => rec.end_turn(result),
        Err(e) => rec.end_turn(&AgentLoopResult {
            llm_error: Some(e.to_string()),
            ..AgentLoopResult::default()
        }),
    }
    rec.finish();
}

/// Finish the run of `agent_id`, so its next turn starts a new run.
pub(super) fn close(state: &mut LoopState<'_>, agent_id: AgentId) {
    state.record_router.detach(agent_id);
    if let Some(live) = state.traces.remove(&agent_id) {
        live.recorder.finish();
    }
}

/// Finish every open run.
pub(super) fn close_all(state: &mut LoopState<'_>) {
    for (agent_id, live) in state.traces.drain() {
        state.record_router.detach(agent_id);
        live.recorder.finish();
    }
}

fn aura_home() -> Option<std::path::PathBuf> {
    aura_config::AuraHome::resolve().ok().map(|h| h.path)
}

/// Store a human label from the graph view on the run or one of its
/// steps.
pub(super) async fn label(state: &LoopState<'_>, run_id: &str, seq: Option<u64>, value: f64) {
    let reward = Reward {
        target: seq.map_or(RewardTarget::Run, |seq| RewardTarget::Step { seq }),
        kind: RewardKind::HumanExplicit,
        name: "human".into(),
        value,
        source: "tui".into(),
        at_ms: aura_agent_trace::bundle::now_ms(),
        evidence: serde_json::Value::Null,
    };
    let live = state
        .traces
        .values()
        .find(|live| live.recorder.run_id() == run_id);
    let stored = match live {
        Some(live) => {
            live.recorder.add_rewards(std::slice::from_ref(&reward));
            Ok(())
        }
        None => aura_home()
            .ok_or_else(|| "AURA_HOME is not resolvable".to_string())
            .and_then(|home| RunBundle::find(&home, run_id).map_err(|e| e.to_string()))
            .and_then(|bundle| {
                bundle
                    .append_rewards(std::slice::from_ref(&reward))
                    .map_err(|e| e.to_string())
            }),
    };
    let target = seq.map_or_else(|| "run".to_string(), |seq| format!("step {seq}"));
    let command = match stored {
        Ok(()) => UiCommand::ShowSuccess(format!("Labeled {target} {value:+}")),
        Err(problem) => UiCommand::ShowWarning(format!("Label not saved: {problem}")),
    };
    let _ = state.commands.send(command).await;
}

/// `/graph [id]`: the active conversation's live graph, or a recorded
/// run's.
pub(super) async fn open_graph(state: &LoopState<'_>, id: Option<String>) {
    let load = match id {
        None => match state.traces.get(&state.active) {
            Some(live) => {
                live.recorder.flush();
                match RunTrace::load(&RunBundle::at(live.recorder.dir())) {
                    Ok(trace) => Ok((trace.meta.run_id.clone(), full_view(&trace.graph()))),
                    Err(_) => {
                        let g = live.graph.lock().unwrap_or_else(PoisonError::into_inner);
                        Ok((live.recorder.run_id(), full_view(&g)))
                    }
                }
            }
            None => Err(
                "No run recorded for this conversation yet. Send a message, \
                         or open a recorded run with /graph <id> (see /runs)."
                    .to_string(),
            ),
        },
        Some(id) => aura_home()
            .ok_or_else(|| "AURA_HOME is not resolvable".to_string())
            .and_then(|home| RunBundle::find(&home, &id).map_err(|e| e.to_string()))
            .and_then(|bundle| RunTrace::load(&bundle).map_err(|e| e.to_string()))
            .map(|trace| (trace.meta.run_id.clone(), full_view(&trace.graph()))),
    };
    let command = match load {
        Ok((run_id, nodes)) => UiCommand::GraphLoad { run_id, nodes },
        Err(problem) => UiCommand::ShowWarning(problem),
    };
    let _ = state.commands.send(command).await;
}

/// `/runs`: the latest recorded runs.
pub(super) async fn list(state: &LoopState<'_>) {
    let Some(home) = aura_home() else {
        return;
    };
    let mut runs = list_runs(&home);
    runs.truncate(RUNS_LISTED);
    let text = if runs.is_empty() {
        "No recorded runs yet.".to_string()
    } else {
        format!(
            "{}\nOpen one with /graph <run>; more with `aura runs show <run>`.",
            report::runs_table(&runs).trim_end()
        )
    };
    show_system_message(state, text).await;
}

/// `/status`: the scorecard of the active conversation's run.
pub(super) async fn status_report(state: &LoopState<'_>) {
    let text = match state.traces.get(&state.active) {
        Some(live) => {
            live.recorder.flush();
            match RunTrace::load(live.recorder.bundle()) {
                Ok(trace) => report::scorecard_text(&trace.scorecard())
                    .trim_end()
                    .to_string(),
                Err(e) => format!("Run trace unreadable: {e}"),
            }
        }
        None => "No run recorded for this conversation yet.".to_string(),
    };
    show_system_message(state, text).await;
}
