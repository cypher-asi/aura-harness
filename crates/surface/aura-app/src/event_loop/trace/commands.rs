//! The `/graph`, `/runs` and `/status` commands, and labels from the graph
//! view.

use std::sync::PoisonError;

use aura_agent_trace::bundle::list_runs;
use aura_agent_trace::{report, RunTrace};
use aura_core_types::trace::{Reward, RewardKind, RewardTarget};
use aura_ui::UiCommand;

use super::super::handlers::show_system_message;
use super::super::LoopState;
use super::{full_view, RUNS_LISTED};

/// Store a human label from the graph view on the run or one of its
/// steps.
pub(crate) async fn label(state: &LoopState<'_>, run_id: &str, seq: Option<u64>, value: f64) {
    let reward = Reward {
        target: seq.map_or(RewardTarget::Run, |seq| RewardTarget::Step { seq }),
        kind: RewardKind::HumanExplicit,
        name: "human".into(),
        value,
        source: "tui".into(),
        at_ms: aura_agent_trace::bundle::now_ms(),
        evidence: serde_json::Value::Null,
    };
    let stored = state.hub.traces.add_reward(run_id, &reward);
    let target = seq.map_or_else(|| "run".to_string(), |seq| format!("step {seq}"));
    let command = match stored {
        Ok(()) => UiCommand::ShowSuccess(format!("Labeled {target} {value:+}")),
        Err(problem) => UiCommand::ShowWarning(format!("Label not saved: {problem}")),
    };
    let _ = state.commands.send(command).await;
}

/// `/graph [id]`: the active conversation's live graph, else its latest
/// stored run, or a recorded run by id.
pub(crate) async fn open_graph(state: &LoopState<'_>, id: Option<String>) {
    let traces = &state.hub.traces;
    let load = match id {
        None => match traces.live_graph(state.active) {
            Some((rec, graph)) => {
                rec.flush();
                match RunTrace::load(rec.bundle()) {
                    Ok(trace) => Ok((
                        trace.meta.run_id.clone(),
                        full_view(&traces.graph_of(&trace)),
                    )),
                    Err(_) => {
                        let g = graph.lock().unwrap_or_else(PoisonError::into_inner);
                        Ok((rec.run_id(), full_view(&g)))
                    }
                }
            }
            None => traces
                .latest(state.active)
                .map(|trace| {
                    (
                        trace.meta.run_id.clone(),
                        full_view(&traces.graph_of(&trace)),
                    )
                })
                .ok_or_else(|| {
                    "No run recorded for this conversation yet. Send a message, \
                     or open a recorded run with /graph <id> (see /runs)."
                        .to_string()
                }),
        },
        Some(id) => traces.load(&id).map(|trace| {
            (
                trace.meta.run_id.clone(),
                full_view(&traces.graph_of(&trace)),
            )
        }),
    };
    let command = match load {
        Ok((run_id, nodes)) => UiCommand::GraphLoad { run_id, nodes },
        Err(problem) => UiCommand::ShowWarning(problem),
    };
    let _ = state.commands.send(command).await;
}

/// `/runs`: the latest recorded runs.
pub(crate) async fn list(state: &LoopState<'_>) {
    let Some(home) = state.hub.traces.home.as_deref() else {
        return;
    };
    let mut runs = list_runs(home);
    runs.reverse();
    runs.truncate(RUNS_LISTED);
    runs.reverse();
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

/// `/status`: the scorecard of the active conversation's run, else of
/// its latest stored run.
pub(crate) async fn status_report(state: &LoopState<'_>) {
    let traces = &state.hub.traces;
    let trace = match traces.recorder(state.active) {
        Some(rec) => {
            rec.flush();
            RunTrace::load(rec.bundle()).map_err(|e| format!("Run trace unreadable: {e}"))
        }
        None => traces
            .latest(state.active)
            .ok_or_else(|| "No run recorded for this conversation yet.".to_string()),
    };
    let text = match trace {
        Ok(trace) => report::scorecard_text(&trace.scorecard())
            .trim_end()
            .to_string(),
        Err(problem) => problem,
    };
    show_system_message(state, text).await;
}
