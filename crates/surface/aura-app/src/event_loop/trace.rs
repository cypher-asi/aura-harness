//! Run traces for TUI conversations.
//!
//! Each agent's conversation is one run under `AURA_HOME/runs/`, mirrored
//! into the agent-state database. Root agents' runs start on their first
//! turn, `task` spawns' and hosted agents' on theirs, linked to the run of
//! the agent that spawned them; a run is finished when the session is
//! reset or the TUI quits, and a scorecard is written after every turn so
//! `aura runs` sees live data. A live [`RunGraph`] per run feeds `/graph`
//! and the agent's TRACE tab. An agent without a live run shows its
//! latest stored run there instead.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex, PoisonError};

use aura_agent::{AgentError, AgentLoopEvent, AgentLoopResult};
use aura_agent_trace::bundle::{child_runs, list_run_dirs, list_runs, RunBundle};
use aura_agent_trace::graph::{GraphNode, NodeStatus};
use aura_agent_trace::{
    db, report, run_log, Activity, DbSink, RecordRouter, RunGraph, RunLog, RunMeta, RunRecorder,
    RunSource, RunTrace,
};
use aura_core_types::trace::{Reward, RewardKind, RewardTarget, StepAction};
use aura_core_types::{AgentId, SubagentExit};
use aura_engine::{HostedTurn, TurnObserver};
use aura_store_db::TraceStore;
use aura_ui_terminal::events::{TraceNode, TraceNodeStatus};
use aura_ui_terminal::UiCommand;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::warn;

use super::handlers::show_system_message;
use super::LoopState;

/// Runs shown by `/runs`.
const RUNS_LISTED: usize = 15;

/// An open run: its recorder and live graph.
struct LiveRun {
    recorder: Arc<RunRecorder>,
    graph: Arc<Mutex<RunGraph>>,
    /// A hosted agent's turn is running.
    in_turn: bool,
    /// The host removed the agent; close the run once its turn ends.
    removed: bool,
}

/// Every agent's open run, and where runs are stored.
pub struct Traces {
    /// `AURA_HOME`, where run bundles go; `None` turns recording off.
    home: Option<PathBuf>,
    store: Option<Arc<TraceStore>>,
    router: Option<Arc<RecordRouter>>,
    commands: mpsc::Sender<UiCommand>,
    live: Mutex<HashMap<AgentId, LiveRun>>,
    /// Agents whose latest stored run went to their TRACE tab.
    shown: Mutex<HashSet<AgentId>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Traces {
    /// Runs recorded under `home`, mirrored into `store` when given, with
    /// kernel record entries joined through `router`, and graph updates
    /// sent on `commands`.
    #[must_use]
    pub fn new(
        home: Option<PathBuf>,
        store: Option<Arc<TraceStore>>,
        router: Option<Arc<RecordRouter>>,
        commands: mpsc::Sender<UiCommand>,
    ) -> Arc<Self> {
        Arc::new(Self {
            home,
            store,
            router,
            commands,
            live: Mutex::new(HashMap::new()),
            shown: Mutex::new(HashSet::new()),
        })
    }

    /// The open run of `agent`, else a new one described by `meta`. `None`
    /// when tracing is off or the bundle can't be created.
    pub(crate) fn start(&self, agent: AgentId, mut meta: RunMeta) -> Option<Arc<RunRecorder>> {
        let mut live = lock(&self.live);
        if let Some(run) = live.get(&agent) {
            return Some(Arc::clone(&run.recorder));
        }
        meta.agent_id = Some(agent.to_hex());
        meta.task_id.get_or_insert_with(|| task_id(agent));
        let home = self.home.as_deref()?;
        let recorder = match RunRecorder::start(home, meta, aura_config::trace().level) {
            Ok(recorder) => recorder?,
            Err(e) => {
                warn!(error = %e, "Run trace disabled: could not create bundle");
                return None;
            }
        };
        if let Some(store) = &self.store {
            recorder.add_sink(DbSink::new(Arc::clone(store)));
        }
        let graph = Arc::new(Mutex::new(RunGraph::new(&recorder.meta())));
        subscribe_graph(&recorder, agent, Arc::clone(&graph), self.commands.clone());
        if let Some(parent_run) = recorder.meta().spawned_by {
            let parent = live
                .iter()
                .find(|(_, run)| run.recorder.run_id() == parent_run)
                .map(|(id, run)| (*id, Arc::clone(&run.graph)));
            if let Some((parent_agent, parent_graph)) = parent {
                link_to_parent(
                    &recorder,
                    (parent_agent, parent_run),
                    parent_graph,
                    self.commands.clone(),
                );
            }
        }
        if let Some(router) = &self.router {
            router.attach(agent, &recorder);
        }
        live.insert(
            agent,
            LiveRun {
                recorder: Arc::clone(&recorder),
                graph,
                in_turn: false,
                removed: false,
            },
        );
        Some(recorder)
    }

    /// The open run of `agent`.
    pub(crate) fn recorder(&self, agent: AgentId) -> Option<Arc<RunRecorder>> {
        lock(&self.live)
            .get(&agent)
            .map(|run| Arc::clone(&run.recorder))
    }

    /// What the open run of `agent` is doing, and whether a turn of it
    /// is under way (only hosted turns say so).
    pub(crate) fn activity(&self, agent: AgentId) -> Option<(Activity, bool)> {
        lock(&self.live)
            .get(&agent)
            .map(|run| (run.recorder.activity(), run.in_turn))
    }

    /// Id of the open run of `agent`.
    pub(crate) fn run_id_of(&self, agent: AgentId) -> Option<String> {
        self.recorder(agent).map(|rec| rec.run_id())
    }

    fn live_graph(&self, agent: AgentId) -> Option<(Arc<RunRecorder>, Arc<Mutex<RunGraph>>)> {
        lock(&self.live)
            .get(&agent)
            .map(|run| (Arc::clone(&run.recorder), Arc::clone(&run.graph)))
    }

    fn live_run(&self, run_id: &str) -> Option<Arc<RunRecorder>> {
        lock(&self.live)
            .values()
            .find(|run| run.recorder.run_id() == run_id)
            .map(|run| Arc::clone(&run.recorder))
    }

    /// Finish the run of `agent`, so its next turn starts a new run.
    pub(crate) fn close(&self, agent: AgentId) {
        let run = lock(&self.live).remove(&agent);
        if let Some(router) = &self.router {
            router.detach(agent);
        }
        if let Some(run) = run {
            run.recorder.finish();
        }
    }

    /// Finish every open run.
    pub(crate) fn close_all(&self) {
        let runs: Vec<(AgentId, LiveRun)> = lock(&self.live).drain().collect();
        for (agent, run) in runs {
            if let Some(router) = &self.router {
                router.detach(agent);
            }
            run.recorder.finish();
        }
    }

    /// The latest stored run of `agent`: from the database, else its
    /// newest bundle, which is then copied into the database.
    fn latest(&self, agent: AgentId) -> Option<RunTrace> {
        let hex = agent.to_hex();
        if let Some(store) = &self.store {
            match db::latest_of(store, &hex) {
                Ok(Some(trace)) => return Some(trace),
                Ok(None) => {}
                Err(e) => warn!(error = %e, "Could not read stored runs"),
            }
        }
        let bundle = latest_bundle_of(self.home.as_deref()?, &hex)?;
        match &self.store {
            Some(store) => db::import(store, &bundle).ok(),
            None => RunTrace::load(&bundle).ok(),
        }
    }

    /// Show `agent`'s latest stored run in its TRACE tab, once, when it
    /// has no live run.
    pub(crate) async fn send_history(&self, agent: AgentId) {
        if let Some(history) = self.history_of(agent) {
            let _ = self.commands.send(history).await;
        }
    }

    /// What [`Self::send_history`] sends, if anything. Reads the stored
    /// run, so it is called off the event loop.
    pub(crate) fn history_of(&self, agent: AgentId) -> Option<UiCommand> {
        if lock(&self.live).contains_key(&agent) || !lock(&self.shown).insert(agent) {
            return None;
        }
        let trace = self.latest(agent)?;
        let nodes = full_view(&self.graph_of(&trace));
        (!nodes.is_empty()).then(|| UiCommand::ForAgent {
            agent_id: agent.to_hex(),
            command: Box::new(UiCommand::GraphUpsert {
                run_id: trace.meta.run_id.clone(),
                nodes,
            }),
        })
    }

    /// Store `reward` on run `run_id`, open or not.
    fn add_reward(&self, run_id: &str, reward: &Reward) -> Result<(), String> {
        if let Some(rec) = self.live_run(run_id) {
            rec.add_rewards(std::slice::from_ref(reward));
            return Ok(());
        }
        let bundle = self.find_bundle(run_id);
        let in_store = self
            .store
            .as_ref()
            .is_some_and(|store| store.meta(run_id).ok().flatten().is_some());
        if let Ok(bundle) = &bundle {
            bundle
                .append_rewards(std::slice::from_ref(reward))
                .map_err(|e| e.to_string())?;
        }
        if let (Some(store), true) = (&self.store, in_store) {
            let bytes = serde_json::to_vec(reward).map_err(|e| e.to_string())?;
            store
                .append_rewards(run_id, &[bytes])
                .map_err(|e| e.to_string())?;
        }
        match bundle {
            Ok(_) => Ok(()),
            Err(_) if in_store => Ok(()),
            Err(problem) => Err(problem),
        }
    }

    /// A stored run by id: open, in the database, or a bundle.
    fn load(&self, run_id: &str) -> Result<RunTrace, String> {
        if let Some(rec) = self.live_run(run_id) {
            rec.flush();
            return RunTrace::load(rec.bundle()).map_err(|e| e.to_string());
        }
        if let Some(store) = &self.store {
            if let Ok(Some(trace)) = db::load(store, run_id) {
                return Ok(trace);
            }
        }
        self.find_bundle(run_id)
            .and_then(|bundle| RunTrace::load(&bundle).map_err(|e| e.to_string()))
    }

    /// `trace`'s graph, with the runs it spawned linked to their calls.
    fn graph_of(&self, trace: &RunTrace) -> RunGraph {
        let mut graph = trace.graph();
        if let Some(home) = self.home.as_deref() {
            graph.link_child_runs(&child_runs(home, &trace.meta.run_id));
        }
        graph
    }

    fn find_bundle(&self, run_id: &str) -> Result<RunBundle, String> {
        let home = self
            .home
            .as_deref()
            .ok_or_else(|| "AURA_HOME is not resolvable".to_string())?;
        RunBundle::find(home, run_id).map_err(|e| e.to_string())
    }

    /// Directory of run `run_id`'s bundle under `home`, where it is or
    /// where it goes.
    fn dir_of(&self, home: &Path, run_id: &str) -> PathBuf {
        self.find_bundle(run_id).map_or_else(
            |_| aura_agent_trace::bundle::runs_dir(home).join(run_id),
            |bundle| bundle.dir().to_path_buf(),
        )
    }

    /// The log of `agent` (named `name`): its open run, else its latest
    /// stored one, with the runs of every agent it spawned and those of
    /// the agents in `also` (a benchmark's runs), written to the run's
    /// `log.json`. An agent with no run of its own but runs in `also` gets
    /// a log of those, in `AURA_HOME/logs/<agent>/`. Returns the log and
    /// where it was written.
    pub(crate) fn run_log(
        &self,
        agent: AgentId,
        name: &str,
        also: &[AgentId],
    ) -> Result<(RunLog, PathBuf), String> {
        let Some(home) = self.home.as_deref() else {
            return Err("Run logs are off: AURA_HOME could not be found".to_string());
        };
        let own = self.log_of(home, agent)?;
        let mut others = Vec::new();
        for &other in also {
            match self.log_of(home, other) {
                Ok(Some((log, _))) => others.push(log),
                Ok(None) => {}
                Err(e) => {
                    warn!(agent = %other.to_hex(), error = %e, "Could not read a run for the log");
                }
            }
        }
        let (mut log, dir) = match own {
            Some(found) => found,
            None if !others.is_empty() => {
                let hex = agent.to_hex();
                let dir = home.join("logs").join(&hex);
                (run_log::group(&hex, name, &dir, Vec::new()), dir)
            }
            None if aura_config::trace().level == aura_config::TraceLevel::Off => {
                return Err("Run logs are off: AURA_TRACE is off".to_string());
            }
            None => return Err("This agent has no run log yet; send it a prompt".to_string()),
        };
        run_log::adopt(&mut log, others);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = run_log::write(&dir, &log).map_err(|e| e.to_string())?;
        Ok((log, path))
    }

    /// The log of `agent`'s open run, else of its latest stored one, with
    /// the runs it spawned, and the bundle it is in. `None` when it has
    /// no run.
    fn log_of(&self, home: &Path, agent: AgentId) -> Result<Option<(RunLog, PathBuf)>, String> {
        let trace = match self.recorder(agent) {
            Some(rec) => {
                rec.flush();
                RunTrace::load(rec.bundle()).map_err(|e| e.to_string())?
            }
            None => match self.latest(agent) {
                Some(trace) => trace,
                None => return Ok(None),
            },
        };
        let dir = self.dir_of(home, &trace.meta.run_id);
        let load = |run_id: &str| {
            let trace = self.load(run_id).ok()?;
            Some((trace, self.dir_of(home, run_id)))
        };
        Ok(Some((run_log::build(Some(home), &trace, &dir, &load), dir)))
    }
}

/// Records hosted agents' turns in their own runs.
impl TurnObserver for Traces {
    fn turn_started(&self, turn: &HostedTurn<'_>) {
        let mut meta = RunMeta::new(turn.model, RunSource::Tui);
        meta.mode = "spawned".into();
        meta.spawned_by = turn.parent.and_then(|parent| self.run_id_of(parent));
        let Some(rec) = self.start(turn.agent_id, meta) else {
            return;
        };
        if let Some(run) = lock(&self.live).get_mut(&turn.agent_id) {
            run.in_turn = true;
        }
        rec.begin_turn(turn.prompt);
    }

    fn on_event(&self, agent_id: AgentId, event: &AgentLoopEvent) {
        if let Some(rec) = self.recorder(agent_id) {
            rec.on_event(event);
        }
    }

    fn turn_ended(&self, agent_id: AgentId, result: Option<&AgentLoopResult>, exit: &SubagentExit) {
        let Some(rec) = self.recorder(agent_id) else {
            return;
        };
        match result {
            Some(result) => rec.end_turn(result),
            None => rec.end_turn(&AgentLoopResult {
                llm_error: Some(exit_reason(exit)),
                ..AgentLoopResult::default()
            }),
        }
        rec.finish();
        let removed = lock(&self.live).get_mut(&agent_id).is_some_and(|run| {
            run.in_turn = false;
            run.removed
        });
        if removed {
            self.forget(agent_id);
        }
    }

    fn agent_removed(&self, agent_id: AgentId) {
        let in_turn = lock(&self.live).get_mut(&agent_id).is_some_and(|run| {
            run.removed = true;
            run.in_turn
        });
        if !in_turn {
            self.forget(agent_id);
        }
    }
}

impl Traces {
    /// Drop a hosted agent's run, already finished after its last turn.
    fn forget(&self, agent: AgentId) {
        let run = lock(&self.live).remove(&agent);
        if let Some(router) = &self.router {
            router.detach(agent);
        }
        drop(run);
    }
}

fn exit_reason(exit: &SubagentExit) -> String {
    match exit {
        SubagentExit::Completed => "completed".into(),
        SubagentExit::Failed { reason } => reason.clone(),
        SubagentExit::Cancelled => "cancelled".into(),
        SubagentExit::Timeout => "timed out".into(),
        SubagentExit::Rejected { reason } => format!("rejected: {reason}"),
    }
}

/// Task id of a terminal agent's runs.
pub(crate) fn task_id(agent: AgentId) -> String {
    format!("tui:{}", agent.to_hex())
}

/// The newest bundle under `home` that records the agent with hex id
/// `agent`.
fn latest_bundle_of(home: &Path, agent: &str) -> Option<RunBundle> {
    list_run_dirs(home).into_iter().rev().find_map(|dir| {
        let bundle = RunBundle::at(dir);
        let meta = bundle.read_meta().ok()?;
        (db::owner_of(&meta).as_deref() == Some(agent)).then_some(bundle)
    })
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
    state: &LoopState<'_>,
    session: &super::agents::AgentSession,
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
fn subscribe_graph(
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
fn link_to_parent(
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
type GraphSender = Arc<dyn Fn(&RunGraph, &[String]) + Send + Sync>;

fn graph_sender(run_id: String, agent: AgentId, commands: mpsc::Sender<UiCommand>) -> GraphSender {
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
pub(super) async fn open_graph(state: &LoopState<'_>, id: Option<String>) {
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
pub(super) async fn list(state: &LoopState<'_>) {
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
pub(super) async fn status_report(state: &LoopState<'_>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_bundle_matches_the_agent_by_id_or_terminal_task() {
        let home = tempfile::tempdir().unwrap();
        let start = |agent_id: Option<&str>, task: Option<&str>, title: &str| {
            let mut meta = RunMeta::new("m", RunSource::Tui);
            meta.agent_id = agent_id.map(str::to_string);
            meta.task_id = task.map(str::to_string);
            meta.title = title.into();
            let rec =
                RunRecorder::start(home.path(), meta, aura_config::trace::TraceLevel::Decisions)
                    .unwrap()
                    .unwrap();
            rec.finish();
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        start(None, Some("tui:aa"), "old");
        start(Some("aa"), None, "new");
        start(Some("bb"), None, "other");

        let found = latest_bundle_of(home.path(), "aa").unwrap();
        assert_eq!(found.read_meta().unwrap().title, "new");
        assert!(latest_bundle_of(home.path(), "cc").is_none());
    }

    #[tokio::test]
    async fn hosted_turns_are_recorded_and_shown_after_a_restart() {
        let home = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let rocks = aura_store_db::RocksStore::open(db.path(), false).unwrap();
        let store = Arc::new(TraceStore::for_store(&rocks));
        let (tx, mut rx) = mpsc::channel(64);
        let traces = Traces::new(
            Some(home.path().to_path_buf()),
            Some(Arc::clone(&store)),
            None,
            tx.clone(),
        );
        let (parent, child) = (AgentId::generate(), AgentId::generate());
        let parent_run = traces
            .start(parent, RunMeta::new("m", RunSource::Tui))
            .unwrap()
            .run_id();

        traces.turn_started(&HostedTurn {
            agent_id: child,
            parent: Some(parent),
            name: "helper",
            model: "m",
            prompt: "count files",
        });
        let child_run = traces.run_id_of(child).unwrap();
        traces.agent_removed(child);
        assert!(traces.recorder(child).is_some(), "kept until its turn ends");
        traces.turn_ended(
            child,
            Some(&AgentLoopResult::default()),
            &SubagentExit::Completed,
        );
        assert!(traces.recorder(child).is_none());
        traces.close_all();
        while rx.try_recv().is_ok() {}

        let stored = db::load(&store, &child_run).unwrap().unwrap();
        assert_eq!(stored.meta.spawned_by.as_deref(), Some(parent_run.as_str()));
        assert_eq!(stored.meta.title, "count files");

        let restarted = Traces::new(Some(home.path().to_path_buf()), Some(store), None, tx);
        restarted.send_history(child).await;
        match rx.try_recv().unwrap() {
            UiCommand::ForAgent { agent_id, command } => {
                assert_eq!(agent_id, child.to_hex());
                assert!(
                    matches!(*command, UiCommand::GraphUpsert { run_id, .. } if run_id == child_run)
                );
            }
            other => panic!("unexpected {other:?}"),
        }
        restarted.send_history(child).await;
        assert!(rx.try_recv().is_err(), "history is sent once");
    }

    #[test]
    fn a_spawned_run_is_linked_to_the_call_in_the_parents_live_graph() {
        use aura_core_types::trace::{
            Actor, ModelAction, Observation, PolicyRef, Step, StepLinks, ToolCallAction, ToolIntent,
        };
        let home = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        let traces = Traces::new(Some(home.path().to_path_buf()), None, None, tx);
        let (parent, child) = (AgentId::generate(), AgentId::generate());
        traces
            .start(parent, RunMeta::new("m", RunSource::Tui))
            .unwrap();
        let (_, graph) = traces.live_graph(parent).unwrap();
        let input = serde_json::json!({"prompt": "count files"});
        lock(&graph).apply(&Step {
            run_id: String::new(),
            seq: 1,
            parent: None,
            turn: 1,
            iteration: Some(0),
            at_ms: 0,
            actor: Actor::Model,
            observation: Observation::None,
            action: StepAction::Model(ModelAction {
                tool_calls: vec![ToolCallAction {
                    tool_use_id: "t1".into(),
                    name: "task".into(),
                    intent: ToolIntent::classify("task", &input),
                    input,
                }],
                ..ModelAction::default()
            }),
            policy: PolicyRef::default(),
            links: StepLinks::default(),
            usage: None,
            duration_ms: None,
        });
        while rx.try_recv().is_ok() {}

        traces.turn_started(&HostedTurn {
            agent_id: child,
            parent: Some(parent),
            name: "helper",
            model: "m",
            prompt: "count files",
        });
        let child_run = traces.run_id_of(child).unwrap();
        let call = lock(&graph).node("tc:t1").unwrap().meta["child_run"].clone();
        assert_eq!(call, serde_json::json!(child_run));
        let sent = std::iter::from_fn(|| rx.try_recv().ok()).any(|c| match c {
            UiCommand::ForAgent { agent_id, command } => {
                agent_id == parent.to_hex()
                    && matches!(*command, UiCommand::GraphUpsert { nodes, .. }
                        if nodes.iter().any(|n| n.id == "tc:t1"
                            && n.meta.iter().any(|(k, v)| k == "child_run" && *v == child_run)))
            }
            _ => false,
        });
        assert!(sent, "the parent's graph view hears about the link");
        traces.close_all();
    }
}
