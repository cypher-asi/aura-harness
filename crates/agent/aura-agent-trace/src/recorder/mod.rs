//! Live run recorder.
//!
//! [`StepBuilder`] turns agent-loop events into [`Step`]s; it is pure
//! so it can be tested and reused by replays. [`RunRecorder`] wraps it
//! with a bundle on disk and a writer thread. Recording never fails or
//! blocks the run: when the writer falls behind, steps are dropped and
//! counted instead.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use aura_agent::{AgentLoopEvent, AgentLoopResult};
use aura_config::TraceLevel;
use aura_core_types::trace::{Reward, RewardKind, RewardTarget, Step};
use aura_store_snapshot::{FsSnapshotStore, SnapshotStore};
use serde_json::Value;

use crate::activity::Activity;
use crate::bundle::{self, now_ms, RunBundle, RunMeta, STEPS_FILE};
use crate::records::RecordRow;
use crate::scorecard::RunScorecard;
use crate::{RunTrace, TraceError};

mod builder;
mod touches;

pub use builder::{StepBuilder, EXCERPT_CHARS};
pub use touches::file_touches;

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
    activity: Mutex<Activity>,
    tx: Mutex<Option<SyncSender<WriteOp>>>,
    writer: Mutex<Option<JoinHandle<()>>>,
    listeners: Mutex<Vec<Listener>>,
    record_listeners: Mutex<Vec<RecordListener>>,
    records_in_flight: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    sinks: Sinks,
    dropped: AtomicU64,
    /// Held while the scorecard and the log are written.
    writing: Mutex<()>,
    /// End time noted by [`Self::finish_later`] for the next write; 0
    /// when none is due.
    due_ms: AtomicU64,
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
            activity: Mutex::new(Activity::default()),
            tx: Mutex::new(Some(tx)),
            writer: Mutex::new(Some(writer)),
            listeners: Mutex::new(Vec::new()),
            record_listeners: Mutex::new(Vec::new()),
            records_in_flight: Mutex::new(None),
            sinks,
            dropped: AtomicU64::new(0),
            writing: Mutex::new(()),
            due_ms: AtomicU64::new(0),
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

    /// Call `wait` before a turn ends or the run flushes, to take in the
    /// record rows still on their way to [`Self::on_record`].
    pub fn wait_for_records(&self, wait: impl Fn() + Send + Sync + 'static) {
        *lock(&self.records_in_flight) = Some(Box::new(wait));
    }

    fn take_records_in_flight(&self) {
        if let Some(wait) = lock(&self.records_in_flight).as_ref() {
            wait();
        }
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
                meta.title = aura_core_types::text::truncate_with_ellipsis(
                    prompt.lines().next().unwrap_or(""),
                    80,
                );
                if let Err(e) = self.bundle.write_meta(&meta) {
                    tracing::debug!(error = %e, "trace meta write failed");
                }
            }
            untitled
        };
        if titled {
            self.push_meta();
        }
        let at_ms = now_ms();
        lock(&self.activity).turn_started(at_ms);
        let step = lock(&self.builder).user_prompt(prompt, at_ms);
        self.emit(step);
    }

    /// What the run is doing right now.
    #[must_use]
    pub fn activity(&self) -> Activity {
        lock(&self.activity).clone()
    }

    /// Record one loop event.
    pub fn on_event(&self, event: &AgentLoopEvent) {
        let at_ms = now_ms();
        lock(&self.activity).on_event(event, at_ms);
        let step = lock(&self.builder).on_event(event, at_ms);
        if let Some(step) = step {
            self.emit(step);
        }
    }

    /// Record a prompt Gate sent within the turn under way.
    pub fn follow_up(&self, text: &str) {
        let step = lock(&self.builder).follow_up(text, now_ms());
        self.emit(step);
    }

    /// Record the end of a turn.
    pub fn end_turn(&self, result: &AgentLoopResult) {
        self.take_records_in_flight();
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
        self.take_records_in_flight();
        let (ack_tx, ack_rx) = mpsc::sync_channel(1);
        let sent = lock(&self.tx)
            .as_ref()
            .is_some_and(|tx| tx.send(WriteOp::Flush(ack_tx)).is_ok());
        if sent {
            let _ = ack_rx.recv();
        }
    }

    /// Flush, stamp the end time, and write the scorecard and the log.
    pub fn finish(&self) -> Option<RunScorecard> {
        let _writing = lock(&self.writing);
        self.due_ms.store(0, Ordering::Release);
        self.write_up(now_ms())
    }

    /// Note that the run may end here, for [`Self::finish_due`] to do
    /// what [`Self::finish`] does off the caller's thread: the log is
    /// rebuilt from the whole run and the runs it spawned, which takes
    /// seconds for a long one. Returns whether that call is still to be
    /// made; not while an earlier one waits to start, as it covers this
    /// one too.
    #[must_use]
    pub fn finish_later(&self) -> bool {
        self.due_ms.swap(now_ms(), Ordering::AcqRel) == 0
    }

    /// What [`Self::finish`] does, stamped with the end time the latest
    /// [`Self::finish_later`] noted, when one was noted since the last
    /// write.
    pub fn finish_due(&self) {
        let _writing = lock(&self.writing);
        let ended_ms = self.due_ms.swap(0, Ordering::AcqRel);
        if ended_ms != 0 {
            self.write_up(ended_ms);
        }
    }

    fn write_up(&self, ended_ms: u64) -> Option<RunScorecard> {
        self.flush();
        self.update_meta(|m| m.ended_at_ms = Some(ended_ms));
        let trace = RunTrace::load(&self.bundle).ok();
        let card = trace.as_ref().map(RunTrace::scorecard);
        if let Some(trace) = &trace {
            let dir = self.bundle.dir();
            let home = crate::run_log::home_of(dir);
            let load =
                |run_id: &str| home.and_then(|home| crate::run_log::bundle_loader(home)(run_id));
            let log = crate::run_log::build(home, trace, dir, &load);
            if let Err(e) = crate::run_log::write(dir, &log) {
                tracing::debug!(error = %e, "trace log write failed");
            }
        }
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
mod tests;
