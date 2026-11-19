//! Live run recorder.
//!
//! [`StepBuilder`] turns agent-loop events into [`Step`]s; it is pure
//! so it can be tested and reused by replays. [`RunRecorder`] wraps it
//! with a bundle on disk and a writer thread. Recording never fails or
//! blocks the run: when the writer falls behind, steps are dropped and
//! counted instead.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use aura_agent::{AgentLoopEvent, AgentLoopResult, DebugEvent};
use aura_config::{HarnessParams, TraceLevel};
use aura_core_types::trace::{
    Actor, DecisionSource, FileTouch, HarnessAction, HarnessDecision, ModelExchange, Observation,
    PolicyKind, PolicyRef, Reward, RewardKind, RewardTarget, Step, StepAction, StepLinks,
};
use aura_store_snapshot::{FsSnapshotStore, SnapshotStore};
use serde_json::{json, Value};

use crate::bundle::{self, now_ms, RunBundle, RunMeta, STEPS_FILE};
use crate::health::{duration, APPROVAL_GUESS_MS, DEFAULT_SLOW_MS};
use crate::records::RecordRow;
use crate::scorecard::RunScorecard;
use crate::{RunTrace, TraceError};

/// Characters of tool output kept inline on a step.
pub const EXCERPT_CHARS: usize = 400;

/// Builds [`Step`]s from agent-loop events.
pub struct StepBuilder {
    run_id: String,
    seq: u64,
    turn: u32,
    iteration: Option<u32>,
    harness_params: Value,
    /// tool_use_id → (model step seq, tool name, input)
    calls: HashMap<String, (u64, String, Value)>,
    /// tool_use_id → unix ms the tool started
    started: HashMap<String, u64>,
    /// tool_use_id → unix ms the kernel recorded its proposal, until the
    /// policy decides it
    proposed: HashMap<String, u64>,
    /// Harness steps since the last model step; they shaped its input.
    pending_causes: Vec<u64>,
    cache_hits: HashSet<String>,
    blocked: HashSet<String>,
    last_model_seq: Option<u64>,
    blobs: Option<Arc<FsSnapshotStore>>,
}

impl StepBuilder {
    /// Builder for `run_id`. `blobs` stores full tool output when set.
    #[must_use]
    pub fn new(
        run_id: impl Into<String>,
        params: &HarnessParams,
        blobs: Option<Arc<FsSnapshotStore>>,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            seq: 0,
            turn: 0,
            iteration: None,
            harness_params: serde_json::to_value(params).unwrap_or(Value::Null),
            calls: HashMap::new(),
            started: HashMap::new(),
            proposed: HashMap::new(),
            pending_causes: Vec::new(),
            cache_hits: HashSet::new(),
            blocked: HashSet::new(),
            last_model_seq: None,
            blobs,
        }
    }

    /// Steps emitted so far.
    #[must_use]
    pub const fn last_seq(&self) -> u64 {
        self.seq
    }

    /// Current 1-based turn.
    #[must_use]
    pub const fn turn(&self) -> u32 {
        self.turn
    }

    /// Model step of the latest model call, if any.
    #[must_use]
    pub const fn last_model_seq(&self) -> Option<u64> {
        self.last_model_seq
    }

    fn step(&mut self, actor: Actor, action: StepAction, at_ms: u64) -> Step {
        self.seq += 1;
        Step {
            run_id: self.run_id.clone(),
            seq: self.seq,
            parent: None,
            turn: self.turn.max(1),
            iteration: self.iteration,
            at_ms,
            actor,
            observation: Observation::None,
            action,
            policy: PolicyRef::default(),
            links: StepLinks::default(),
            usage: None,
            duration_ms: None,
        }
    }

    /// A user prompt starts a new turn.
    pub fn user_prompt(&mut self, text: &str, at_ms: u64) -> Step {
        self.turn += 1;
        self.iteration = None;
        self.pending_causes.clear();
        let mut step = self.step(
            Actor::User,
            StepAction::UserPrompt {
                text: text.to_string(),
            },
            at_ms,
        );
        step.policy.kind = PolicyKind::Human;
        step.policy.id = "user".into();
        step
    }

    /// Translate one loop event. Most UI events produce nothing.
    pub fn on_event(&mut self, event: &AgentLoopEvent, at_ms: u64) -> Option<Step> {
        match event {
            AgentLoopEvent::ToolStart { id, .. } => {
                self.started.insert(id.clone(), at_ms);
                None
            }
            AgentLoopEvent::ToolResult {
                tool_use_id,
                tool_name,
                content,
                is_error,
                ..
            } => Some(self.tool_result(tool_use_id, tool_name, content, *is_error, at_ms)),
            AgentLoopEvent::StreamReset { reason, .. } => Some(self.harness(
                &HarnessDecision::new(
                    DecisionSource::Retry,
                    "stream_reset",
                    HarnessAction::Retry {
                        attempt: 0,
                        wait_ms: 0,
                        reason: reason.clone(),
                    },
                ),
                at_ms,
            )),
            AgentLoopEvent::TestSuiteWarning {
                passed,
                summary,
                failed_tests,
            } => Some(
                self.harness(
                    &HarnessDecision::new(
                        DecisionSource::SelfReview,
                        "test_suite",
                        HarnessAction::Build {
                            success: *passed,
                            error_count: failed_tests.len(),
                        },
                    )
                    .with_trigger(json!({ "summary": summary, "failed_tests": failed_tests })),
                    at_ms,
                ),
            ),
            AgentLoopEvent::Debug(debug) => self.on_debug(debug, at_ms),
            _ => None,
        }
    }

    fn on_debug(&mut self, event: &DebugEvent, at_ms: u64) -> Option<Step> {
        match event {
            DebugEvent::ModelExchange { exchange, .. } => Some(self.model(exchange, at_ms)),
            DebugEvent::Decision { decision, .. } => Some(self.harness(decision, at_ms)),
            DebugEvent::Retry {
                reason,
                attempt,
                wait_ms,
                ..
            } => Some(self.harness(
                &HarnessDecision::new(
                    DecisionSource::Retry,
                    "provider_retry",
                    HarnessAction::Retry {
                        attempt: *attempt,
                        wait_ms: *wait_ms,
                        reason: reason.clone(),
                    },
                ),
                at_ms,
            )),
            DebugEvent::Blocker {
                kind,
                path,
                message,
                ..
            } => {
                let mut decision = HarnessDecision::new(
                    DecisionSource::Policy,
                    kind.clone(),
                    HarnessAction::Block {
                        tool: String::new(),
                        tool_use_id: None,
                        reason: message.clone(),
                    },
                );
                if let Some(path) = path {
                    decision = decision.with_file(path.clone());
                }
                Some(self.harness(&decision, at_ms))
            }
            DebugEvent::LlmCall { .. } | DebugEvent::Iteration { .. } => None,
        }
    }

    fn model(&mut self, exchange: &ModelExchange, at_ms: u64) -> Step {
        self.iteration = Some(exchange.iteration);
        let mut step = self.step(
            Actor::Model,
            StepAction::Model(exchange.action.clone()),
            at_ms,
        );
        let seq = step.seq;
        for call in &exchange.action.tool_calls {
            self.calls.insert(
                call.tool_use_id.clone(),
                (seq, call.name.clone(), call.input.clone()),
            );
            step.links.tool_use_ids.push(call.tool_use_id.clone());
        }
        step.links.caused_by = std::mem::take(&mut self.pending_causes);
        step.policy = PolicyRef {
            kind: PolicyKind::Model,
            id: exchange.model.clone(),
            version: String::new(),
            params: exchange
                .observation
                .as_ref()
                .map_or(Value::Null, |o| o.params.clone()),
            propensity: None,
        };
        if let Some(obs) = &exchange.observation {
            step.observation = Observation::Model(obs.clone());
        }
        step.usage = Some(exchange.usage);
        step.duration_ms = Some(exchange.latency_ms);
        self.last_model_seq = Some(seq);
        step
    }

    fn harness(&mut self, decision: &HarnessDecision, at_ms: u64) -> Step {
        if decision.iteration.is_some() {
            self.iteration = decision.iteration;
        }
        let mut step = self.step(
            Actor::Harness {
                source: decision.source,
            },
            StepAction::Harness {
                name: decision.name.clone(),
                action: decision.action.clone(),
                alternatives: decision.alternatives.clone(),
            },
            at_ms,
        );
        step.observation = Observation::Harness {
            features: decision.features.clone(),
            trigger: decision.trigger.clone(),
        };
        step.policy = PolicyRef {
            kind: PolicyKind::Harness,
            id: decision.name.clone(),
            version: decision.version.to_string(),
            params: self.harness_params.clone(),
            propensity: None,
        };
        step.links = decision.links.clone();
        match &decision.action {
            HarnessAction::CacheHit { tool_use_id, .. } => {
                self.cache_hits.insert(tool_use_id.clone());
            }
            HarnessAction::Block {
                tool_use_id: Some(id),
                ..
            } => {
                self.blocked.insert(id.clone());
            }
            _ => {}
        }
        step.parent = step
            .links
            .tool_use_ids
            .first()
            .and_then(|id| self.calls.get(id))
            .map(|(seq, _, _)| *seq);
        if step.parent.is_none() {
            self.pending_causes.push(step.seq);
        }
        step
    }

    fn tool_result(
        &mut self,
        tool_use_id: &str,
        tool_name: &str,
        content: &str,
        is_error: bool,
        at_ms: u64,
    ) -> Step {
        let (parent, input) = match self.calls.get(tool_use_id) {
            Some((seq, _, input)) => (Some(*seq), input.clone()),
            None => (None, Value::Null),
        };
        let source = if self.blocked.contains(tool_use_id) {
            "blocked"
        } else if self.cache_hits.contains(tool_use_id) {
            "cache"
        } else {
            "executor"
        };
        let files = file_touches(tool_name, &input, is_error);
        let content_blob = self.blobs.as_ref().map(|store| {
            store
                .put_bytes(content.as_bytes())
                .unwrap_or_else(|_| aura_core_types::trace::blob_id(content.as_bytes()))
        });
        let mut step = self.step(
            Actor::Environment,
            StepAction::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                tool: tool_name.to_string(),
                is_error,
                source: source.to_string(),
                content_chars: content.chars().count(),
                excerpt: excerpt(content, EXCERPT_CHARS),
                content: content_blob,
                files: files.clone(),
            },
            at_ms,
        );
        step.parent = parent;
        step.policy.id = tool_name.to_string();
        step.links.tool_use_ids.push(tool_use_id.to_string());
        step.links.files = files.into_iter().map(|f| f.path).collect();
        step.duration_ms = self
            .started
            .remove(tool_use_id)
            .map(|start| at_ms.saturating_sub(start));
        step
    }

    /// The loop finished a turn.
    pub fn turn_end(&mut self, result: &AgentLoopResult, at_ms: u64) -> Step {
        let mut step = self.step(
            Actor::Environment,
            StepAction::TurnEnd {
                stop_reason: result.stop_reason().to_string(),
                iterations: result.iterations,
                input_tokens: result.total_input_tokens,
                output_tokens: result.total_output_tokens,
                files_changed: result.file_changes.len(),
                error: result.llm_error.clone(),
            },
            at_ms,
        );
        step.links.files = result.file_changes.iter().map(|c| c.path.clone()).collect();
        step.policy.id = "agent_loop".into();
        self.calls.clear();
        self.started.clear();
        self.proposed.clear();
        self.pending_causes.clear();
        step
    }

    /// Translate one kernel record row. A policy decision that waited for
    /// an approval becomes an `approval_wait` step: allowed, flagged when
    /// the wait was long, or blocked when the call was denied.
    pub fn on_record(&mut self, row: &RecordRow) -> Option<Step> {
        let id = row.tool_use_id.as_deref()?;
        match row.tx_type.as_str() {
            "tool_proposal" => {
                self.proposed.insert(id.to_string(), row.at_ms);
                None
            }
            "policy_decision" => {
                let wait_ms = row.at_ms.saturating_sub(self.proposed.remove(id)?);
                if row.approver.is_none() && wait_ms < APPROVAL_GUESS_MS {
                    return None;
                }
                let tool = row.tool.clone().unwrap_or_default();
                let waited = duration(wait_ms);
                let action = if row.verdict.as_deref() == Some("denied") {
                    HarnessAction::Block {
                        tool: tool.clone(),
                        tool_use_id: Some(id.to_string()),
                        reason: format!("denied after waiting {waited} for approval"),
                    }
                } else if wait_ms >= DEFAULT_SLOW_MS {
                    HarnessAction::Warn {
                        message: format!("{tool} waited {waited} for approval"),
                    }
                } else {
                    HarnessAction::Allow
                };
                let decision =
                    HarnessDecision::new(DecisionSource::Policy, "approval_wait", action)
                        .with_tool_use(id)
                        .with_trigger(json!({
                            "tool": tool,
                            "wait_ms": wait_ms,
                            "approver": row.approver,
                            "verdict": row.verdict,
                        }));
                Some(self.harness(&decision, row.at_ms))
            }
            _ => None,
        }
    }
}

/// Files a tool call read or wrote, derived from its input.
#[must_use]
pub fn file_touches(tool: &str, input: &Value, is_error: bool) -> Vec<FileTouch> {
    let Some(path) = input.get("path").and_then(Value::as_str) else {
        return Vec::new();
    };
    let kind = match tool {
        "read_file" | "stat_file" => "read",
        "write_file" => "create",
        "edit_file" | "multi_edit" | "apply_patch" => "modify",
        "delete_file" => "delete",
        _ => return Vec::new(),
    };
    if is_error && kind != "read" {
        return Vec::new();
    }
    let lines = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .map_or(0, |s| u32::try_from(s.lines().count()).unwrap_or(u32::MAX))
    };
    let (added, removed) = match tool {
        "edit_file" => (lines("new_text"), lines("old_text")),
        "write_file" => (lines("content"), 0),
        _ => (0, 0),
    };
    vec![FileTouch {
        path: path.to_string(),
        kind: kind.to_string(),
        added,
        removed,
    }]
}

/// First `max` characters of `text`, with an ellipsis when cut.
#[must_use]
pub fn excerpt(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().nth(max).is_some() {
        out.push('…');
    }
    out
}

enum WriteOp {
    Step(Box<Step>),
    Flush(SyncSender<()>),
}

type Listener = Box<dyn Fn(&Step) + Send + Sync>;
type RecordListener = Box<dyn Fn(&RecordRow) + Send + Sync>;

/// A second home for everything a [`RunRecorder`] writes to its bundle,
/// such as the agent-state database. Calls come from the recorder's
/// threads and must not block for long; failures are the sink's to log.
pub trait TraceSink: Send + Sync {
    /// The run's metadata, on start and whenever it changes.
    fn meta(&self, meta: &RunMeta);
    /// Steps, in sequence order, once they are written to the bundle.
    fn steps(&self, run_id: &str, steps: &[Step]);
    /// Rewards added to the run.
    fn rewards(&self, run_id: &str, rewards: &[Reward]);
    /// Kernel record rows joined to the run.
    fn records(&self, run_id: &str, rows: &[RecordRow]);
    /// The scorecard written when the run finishes.
    fn summary(&self, run_id: &str, card: &RunScorecard);
}

type Sinks = Arc<Mutex<Vec<Arc<dyn TraceSink>>>>;

/// Records one run to `AURA_HOME/runs/<run_id>/`.
pub struct RunRecorder {
    meta: Mutex<RunMeta>,
    bundle: RunBundle,
    level: TraceLevel,
    blobs: Option<Arc<FsSnapshotStore>>,
    builder: Mutex<StepBuilder>,
    tx: Mutex<Option<SyncSender<WriteOp>>>,
    writer: Mutex<Option<JoinHandle<()>>>,
    listeners: Mutex<Vec<Listener>>,
    record_listeners: Mutex<Vec<RecordListener>>,
    sinks: Sinks,
    dropped: AtomicU64,
}

impl std::fmt::Debug for RunRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunRecorder")
            .field("dir", &self.bundle.dir())
            .field("level", &self.level)
            .finish_non_exhaustive()
    }
}

const WRITE_QUEUE: usize = 4096;

impl RunRecorder {
    /// Start recording under `home`. Returns `Ok(None)` when `level` is
    /// [`TraceLevel::Off`].
    ///
    /// # Errors
    ///
    /// Returns an error when the bundle directory can't be created.
    pub fn start(
        home: &Path,
        mut meta: RunMeta,
        level: TraceLevel,
    ) -> Result<Option<Arc<Self>>, TraceError> {
        if level == TraceLevel::Off {
            return Ok(None);
        }
        meta.trace_level = level.as_str().to_string();
        let bundle = RunBundle::create(home, &meta)?;
        let blobs = if level == TraceLevel::Full {
            Some(Arc::new(FsSnapshotStore::open(bundle::blobs_dir(home))?))
        } else {
            None
        };
        let (tx, rx) = mpsc::sync_channel::<WriteOp>(WRITE_QUEUE);
        let path = bundle.path(STEPS_FILE);
        let sinks: Sinks = Arc::default();
        let writer_sinks = Arc::clone(&sinks);
        let run_id = meta.run_id.clone();
        let writer = std::thread::Builder::new()
            .name("aura-trace-writer".into())
            .spawn(move || writer_loop(&path, &rx, &run_id, &writer_sinks))?;
        let builder = StepBuilder::new(meta.run_id.clone(), &meta.params, blobs.clone());
        Ok(Some(Arc::new(Self {
            meta: Mutex::new(meta),
            bundle,
            level,
            blobs,
            builder: Mutex::new(builder),
            tx: Mutex::new(Some(tx)),
            writer: Mutex::new(Some(writer)),
            listeners: Mutex::new(Vec::new()),
            record_listeners: Mutex::new(Vec::new()),
            sinks,
            dropped: AtomicU64::new(0),
        })))
    }

    /// Mirror the run into `sink` as well: what was recorded so far
    /// right away, then everything after.
    ///
    /// Lock order is sinks before meta everywhere. Rewards and record
    /// rows are appended to the bundle under the sinks lock, so none is
    /// both copied here and sent again. Steps may reach a new sink
    /// twice; sinks store them by seq.
    pub fn add_sink(&self, sink: Arc<dyn TraceSink>) {
        self.flush();
        let mut sinks = lock(&self.sinks);
        let run_id = self.run_id();
        sink.meta(&self.meta());
        let steps = self.bundle.read_steps().unwrap_or_default();
        if !steps.is_empty() {
            sink.steps(&run_id, &steps);
        }
        let rewards = self.bundle.read_rewards().unwrap_or_default();
        if !rewards.is_empty() {
            sink.rewards(&run_id, &rewards);
        }
        let records = self.bundle.read_records().unwrap_or_default();
        if !records.is_empty() {
            sink.records(&run_id, &records);
        }
        sinks.push(sink);
    }

    fn push_meta(&self) {
        let sinks = lock(&self.sinks);
        if sinks.is_empty() {
            return;
        }
        let meta = self.meta();
        for sink in sinks.iter() {
            sink.meta(&meta);
        }
    }

    /// Start a recorder with the process-wide trace level and
    /// `AURA_HOME`. Failures are logged and return `None`.
    #[must_use]
    pub fn start_default(meta: RunMeta) -> Option<Arc<Self>> {
        let home = aura_config::aura_home::AuraHome::resolve().ok()?;
        match Self::start(&home.path, meta, aura_config::trace().level) {
            Ok(rec) => rec,
            Err(e) => {
                tracing::warn!(error = %e, "run trace disabled: could not create bundle");
                None
            }
        }
    }

    /// Run id.
    #[must_use]
    pub fn run_id(&self) -> String {
        lock(&self.meta).run_id.clone()
    }

    /// Bundle directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        self.bundle.dir()
    }

    /// Current metadata.
    #[must_use]
    pub fn meta(&self) -> RunMeta {
        lock(&self.meta).clone()
    }

    /// Bundle on disk.
    #[must_use]
    pub const fn bundle(&self) -> &RunBundle {
        &self.bundle
    }

    /// Capture level.
    #[must_use]
    pub const fn level(&self) -> TraceLevel {
        self.level
    }

    /// Blob store to hand to the agent loop so it records exact
    /// requests. `None` below [`TraceLevel::Full`].
    #[must_use]
    pub fn blob_store(&self) -> Option<Arc<dyn SnapshotStore>> {
        self.blobs.clone().map(|s| s as Arc<dyn SnapshotStore>)
    }

    /// Steps dropped because the writer fell behind.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Call `f` with every recorded step (for live views).
    pub fn subscribe(&self, f: impl Fn(&Step) + Send + Sync + 'static) {
        lock(&self.listeners).push(Box::new(f));
    }

    /// Call `f` with every kernel record row joined to the run (for live
    /// views).
    pub fn subscribe_records(&self, f: impl Fn(&RecordRow) + Send + Sync + 'static) {
        lock(&self.record_listeners).push(Box::new(f));
    }

    /// Update metadata (title, mode, task id) after start.
    pub fn update_meta(&self, f: impl FnOnce(&mut RunMeta)) {
        {
            let mut meta = lock(&self.meta);
            f(&mut meta);
            if let Err(e) = self.bundle.write_meta(&meta) {
                tracing::debug!(error = %e, "trace meta write failed");
            }
        }
        self.push_meta();
    }

    fn emit(&self, step: Step) {
        for listener in lock(&self.listeners).iter() {
            listener(&step);
        }
        let tx = lock(&self.tx);
        let Some(tx) = tx.as_ref() else {
            return;
        };
        match tx.try_send(WriteOp::Step(Box::new(step))) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Record a user prompt, starting a turn.
    pub fn begin_turn(&self, prompt: &str) {
        let titled = {
            let mut meta = lock(&self.meta);
            let untitled = meta.title.is_empty();
            if untitled {
                meta.title = crate::recorder::excerpt(prompt.lines().next().unwrap_or(""), 80);
                if let Err(e) = self.bundle.write_meta(&meta) {
                    tracing::debug!(error = %e, "trace meta write failed");
                }
            }
            untitled
        };
        if titled {
            self.push_meta();
        }
        let step = lock(&self.builder).user_prompt(prompt, now_ms());
        self.emit(step);
    }

    /// Record one loop event.
    pub fn on_event(&self, event: &AgentLoopEvent) {
        let step = lock(&self.builder).on_event(event, now_ms());
        if let Some(step) = step {
            self.emit(step);
        }
    }

    /// Record the end of a turn.
    pub fn end_turn(&self, result: &AgentLoopResult) {
        let step = lock(&self.builder).turn_end(result, now_ms());
        self.emit(step);
    }

    /// The user interrupted the run (Esc). Stored as an implicit human
    /// signal on the latest model step.
    pub fn interrupted(&self) {
        let target = lock(&self.builder).last_model_seq();
        let reward = Reward {
            target: target.map_or(RewardTarget::Run, |seq| RewardTarget::Step { seq }),
            kind: RewardKind::HumanImplicit,
            name: "interrupted".into(),
            value: -1.0,
            source: "tui".into(),
            at_ms: now_ms(),
            evidence: Value::Null,
        };
        self.add_rewards(&[reward]);
    }

    /// Append a kernel record row for the record-log join.
    pub fn on_record(&self, row: &RecordRow) {
        {
            let sinks = lock(&self.sinks);
            if let Err(e) = self.bundle.append_records(std::slice::from_ref(row)) {
                tracing::debug!(error = %e, "trace record write failed");
            }
            let run_id = self.run_id();
            for sink in sinks.iter() {
                sink.records(&run_id, std::slice::from_ref(row));
            }
        }
        for listener in lock(&self.record_listeners).iter() {
            listener(row);
        }
        let step = lock(&self.builder).on_record(row);
        if let Some(step) = step {
            self.emit(step);
        }
    }

    /// Append external rewards (verification outcome, human labels).
    pub fn add_rewards(&self, rewards: &[Reward]) {
        let sinks = lock(&self.sinks);
        if let Err(e) = self.bundle.append_rewards(rewards) {
            tracing::debug!(error = %e, "trace reward write failed");
        }
        let run_id = self.run_id();
        for sink in sinks.iter() {
            sink.rewards(&run_id, rewards);
        }
    }

    /// Block until queued steps are on disk.
    pub fn flush(&self) {
        let (ack_tx, ack_rx) = mpsc::sync_channel(1);
        let sent = lock(&self.tx)
            .as_ref()
            .is_some_and(|tx| tx.send(WriteOp::Flush(ack_tx)).is_ok());
        if sent {
            let _ = ack_rx.recv();
        }
    }

    /// Flush, stamp the end time and write the scorecard.
    pub fn finish(&self) -> Option<RunScorecard> {
        self.flush();
        self.update_meta(|m| m.ended_at_ms = Some(now_ms()));
        let card = RunTrace::load(&self.bundle).ok().map(|t| t.scorecard());
        if let Some(card) = &card {
            if let Err(e) = self.bundle.write_summary(card) {
                tracing::debug!(error = %e, "trace summary write failed");
            }
            let sinks = lock(&self.sinks);
            let run_id = self.run_id();
            for sink in sinks.iter() {
                sink.summary(&run_id, card);
            }
        }
        card
    }
}

impl Drop for RunRecorder {
    fn drop(&mut self) {
        drop(lock(&self.tx).take());
        if let Some(handle) = lock(&self.writer).take() {
            let _ = handle.join();
        }
    }
}

fn writer_loop(path: &Path, rx: &mpsc::Receiver<WriteOp>, run_id: &str, sinks: &Sinks) {
    use std::io::Write;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path);
    let Ok(file) = file else {
        tracing::warn!(path = %path.display(), "run trace writer could not open steps file");
        return;
    };
    let mut out = std::io::BufWriter::new(file);
    let mut batch: Vec<Step> = Vec::new();
    let send = |batch: &mut Vec<Step>| {
        if batch.is_empty() {
            return;
        }
        for sink in lock(sinks).iter() {
            sink.steps(run_id, batch);
        }
        batch.clear();
    };
    while let Ok(op) = rx.recv() {
        let mut pending = Some(op);
        while let Some(op) = pending.take() {
            match op {
                WriteOp::Step(step) => {
                    if serde_json::to_writer(&mut out, &*step).is_ok() {
                        let _ = out.write_all(b"\n");
                    }
                    batch.push(*step);
                }
                WriteOp::Flush(ack) => {
                    let _ = out.flush();
                    send(&mut batch);
                    let _ = ack.send(());
                }
            }
            pending = rx.try_recv().ok();
        }
        let _ = out.flush();
        send(&mut batch);
    }
    let _ = out.flush();
    send(&mut batch);
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::trace::{ModelAction, TokenUsage, ToolCallAction, ToolIntent};

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }

    pub(crate) fn exchange(iteration: u32, calls: &[(&str, &str, Value)]) -> AgentLoopEvent {
        AgentLoopEvent::Debug(DebugEvent::ModelExchange {
            timestamp: now(),
            exchange: Box::new(ModelExchange {
                iteration,
                model: "m".into(),
                observation: None,
                action: ModelAction {
                    tool_calls: calls
                        .iter()
                        .map(|(id, name, input)| ToolCallAction {
                            tool_use_id: (*id).into(),
                            name: (*name).into(),
                            input: input.clone(),
                            intent: ToolIntent::classify(name, input),
                        })
                        .collect(),
                    stop_reason: "tool_use".into(),
                    ..ModelAction::default()
                },
                usage: TokenUsage {
                    input: 100,
                    output: 10,
                    cache_write: 0,
                    cache_read: 50,
                },
                latency_ms: 7,
                provider_request_id: None,
            }),
            task_id: None,
        })
    }

    #[test]
    fn builder_links_tools_to_model_and_decisions_to_next_model() {
        let mut b = StepBuilder::new("r", &HarnessParams::default(), None);
        let prompt = b.user_prompt("fix it", 1);
        assert_eq!(prompt.turn, 1);
        let nudge = b
            .on_event(
                &AgentLoopEvent::Debug(DebugEvent::Decision {
                    timestamp: now(),
                    decision: Box::new(
                        HarnessDecision::new(
                            DecisionSource::Steering,
                            "implement_now",
                            HarnessAction::Nudge {
                                kind: "implement_now".into(),
                            },
                        )
                        .at_iteration(0),
                    ),
                    task_id: None,
                }),
                2,
            )
            .unwrap();
        let model = b
            .on_event(
                &exchange(0, &[("t1", "read_file", json!({"path": "a.rs"}))]),
                3,
            )
            .unwrap();
        assert_eq!(model.links.caused_by, vec![nudge.seq]);
        assert_eq!(model.usage.unwrap().cache_read, 50);
        b.on_event(
            &AgentLoopEvent::ToolStart {
                id: "t1".into(),
                name: "read_file".into(),
            },
            4,
        );
        let result = b
            .on_event(
                &AgentLoopEvent::ToolResult {
                    tool_use_id: "t1".into(),
                    tool_name: "read_file".into(),
                    content: "fn main() {}".into(),
                    is_error: false,
                    image: None,
                },
                9,
            )
            .unwrap();
        assert_eq!(result.parent, Some(model.seq));
        assert_eq!(result.duration_ms, Some(5));
        assert_eq!(result.links.files, vec!["a.rs".to_string()]);
        let end = b.turn_end(&AgentLoopResult::default(), 10);
        assert!(matches!(end.action, StepAction::TurnEnd { .. }));
        assert_eq!(end.seq, 5);
    }

    #[test]
    fn approval_waits_become_policy_steps() {
        let row = |tx_type: &str, at_ms: u64, id: &str| RecordRow {
            seq: 0,
            tx_type: tx_type.into(),
            at_ms,
            tool_use_id: Some(id.into()),
            tool: Some("edit_file".into()),
            verdict: None,
            approver: None,
            rule: None,
            reason: None,
        };
        let mut b = StepBuilder::new("r", &HarnessParams::default(), None);
        b.user_prompt("x", 0);
        let model = b
            .on_event(
                &exchange(
                    0,
                    &[
                        ("quick", "edit_file", json!({"path": "a.ts"})),
                        ("slow", "edit_file", json!({"path": "a.ts"})),
                        ("no", "edit_file", json!({"path": "a.ts"})),
                    ],
                ),
                1,
            )
            .unwrap();
        for id in ["quick", "slow", "no"] {
            assert!(b.on_record(&row("tool_proposal", 10, id)).is_none());
        }
        assert!(
            b.on_record(&row("policy_decision", 13, "quick")).is_none(),
            "a decision nobody was asked about is no step"
        );

        let slow = b
            .on_record(&RecordRow {
                verdict: Some("approved".into()),
                approver: Some("user".into()),
                ..row("policy_decision", 244_678, "slow")
            })
            .unwrap();
        assert_eq!(slow.parent, Some(model.seq));
        let StepAction::Harness { name, action, .. } = &slow.action else {
            panic!("expected a harness step, got {:?}", slow.action);
        };
        assert_eq!(name, "approval_wait");
        assert!(
            matches!(action, HarnessAction::Warn { message } if message.contains("4m 04s")),
            "{action:?}"
        );

        let denied = b
            .on_record(&RecordRow {
                verdict: Some("denied".into()),
                ..row("policy_decision", 600_010, "no")
            })
            .unwrap();
        assert!(matches!(
            denied.action,
            StepAction::Harness {
                action: HarnessAction::Block { .. },
                ..
            }
        ));
    }

    #[test]
    fn cache_hits_mark_tool_results() {
        let mut b = StepBuilder::new("r", &HarnessParams::default(), None);
        b.user_prompt("x", 0);
        b.on_event(
            &exchange(0, &[("t1", "read_file", json!({"path": "a"}))]),
            1,
        );
        let hit = b
            .on_event(
                &AgentLoopEvent::Debug(DebugEvent::Decision {
                    timestamp: now(),
                    decision: Box::new(
                        HarnessDecision::new(
                            DecisionSource::Cache,
                            "tool_result_cache",
                            HarnessAction::CacheHit {
                                tool: "read_file".into(),
                                tool_use_id: "t1".into(),
                            },
                        )
                        .with_tool_use("t1"),
                    ),
                    task_id: None,
                }),
                2,
            )
            .unwrap();
        assert_eq!(hit.parent, Some(2));
        let result = b
            .on_event(
                &AgentLoopEvent::ToolResult {
                    tool_use_id: "t1".into(),
                    tool_name: "read_file".into(),
                    content: "x".into(),
                    is_error: false,
                    image: None,
                },
                3,
            )
            .unwrap();
        let StepAction::ToolResult { source, .. } = result.action else {
            panic!("expected tool result");
        };
        assert_eq!(source, "cache");
    }

    #[test]
    fn recorder_writes_bundle_and_summary() {
        let home = tempfile::tempdir().unwrap();
        let meta = RunMeta::new("m", crate::RunSource::Test);
        let rec = RunRecorder::start(home.path(), meta, TraceLevel::Full)
            .unwrap()
            .unwrap();
        assert!(rec.blob_store().is_some());
        let seen = Arc::new(AtomicU64::new(0));
        let seen2 = seen.clone();
        rec.subscribe(move |_| {
            seen2.fetch_add(1, Ordering::Relaxed);
        });
        rec.begin_turn("hello\nworld");
        rec.on_event(&exchange(0, &[]));
        rec.end_turn(&AgentLoopResult::default());
        let card = rec.finish().unwrap();
        assert_eq!(card.model_calls, 1);
        assert_eq!(seen.load(Ordering::Relaxed), 3);
        let meta = rec.bundle().read_meta().unwrap();
        assert_eq!(meta.title, "hello");
        assert!(meta.ended_at_ms.is_some());
        assert_eq!(rec.bundle().read_steps().unwrap().len(), 3);
    }

    #[test]
    fn off_level_records_nothing() {
        let home = tempfile::tempdir().unwrap();
        let meta = RunMeta::new("m", crate::RunSource::Test);
        assert!(RunRecorder::start(home.path(), meta, TraceLevel::Off)
            .unwrap()
            .is_none());
        assert!(bundle::list_run_dirs(home.path()).is_empty());
    }
}
