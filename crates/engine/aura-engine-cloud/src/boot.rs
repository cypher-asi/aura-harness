//! What a machine coming up shows while it happens: the stages of a
//! move to the cloud, a move back or a wake, the facts boxd reports
//! about the machine, and the output of the commands run on it. The app
//! draws these as a boot console in the agent's chat. A stage blocked on
//! a slow call says what it waits on and for how long at most, so a slow
//! boot can be told from a stuck one.
//!
//! ## Invariants
//!
//! - Nothing secret is reported: facts are built from machine info and
//!   names only, and the output of the node claim (whose input carries
//!   tokens) is never streamed.
//! - Reporting never blocks or fails the work it reports on: the channel
//!   is unbounded and a closed receiver is ignored.
//! - A waiting line is cleared when its wait ends, however it ends
//!   (done, failed or dropped).

use std::fmt::Display;
use std::future::Future;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

/// Longest log line kept, in characters.
pub const MAX_LINE_CHARS: usize = 400;

/// A step of bringing an agent's machine up or an agent back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BootStage {
    /// Reach boxd and take back the machines placed agents had.
    Connect,
    /// Hand the owner's plan secrets to boxd.
    Plans,
    /// Hand the project's GitHub token to boxd.
    Repo,
    /// Find or build the project snapshot.
    Snapshot,
    /// Check the org has room for another machine.
    Quota,
    /// Create the machine.
    Create,
    /// Set its bot protection and hibernation backstop.
    Configure,
    /// Give its node the hub's tokens.
    Claim,
    /// Bring the project files up to date on it.
    SyncFiles,
    /// Give its node the agent's record.
    ImportRecord,
    /// Wake a sleeping machine.
    Wake,
    /// Stop the agent writing on its node.
    Seal,
    /// Copy what the node wrote back here.
    CopyRecord,
    /// Apply the machine's changes to the project folder.
    PullChanges,
    /// Destroy the machine.
    Destroy,
}

impl BootStage {
    /// Stable key, for matching updates to a stage.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Plans => "plans",
            Self::Repo => "repo",
            Self::Snapshot => "snapshot",
            Self::Quota => "quota",
            Self::Create => "create",
            Self::Configure => "configure",
            Self::Claim => "claim",
            Self::SyncFiles => "sync_files",
            Self::ImportRecord => "import_record",
            Self::Wake => "wake",
            Self::Seal => "seal",
            Self::CopyRecord => "copy_record",
            Self::PullChanges => "pull_changes",
            Self::Destroy => "destroy",
        }
    }

    /// What the stage does, for the checklist.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Connect => "Connecting to boxd",
            Self::Plans => "Sharing plan sign-ins",
            Self::Repo => "Sharing GitHub access",
            Self::Snapshot => "Preparing project snapshot",
            Self::Quota => "Checking machine quota",
            Self::Create => "Creating machine",
            Self::Configure => "Configuring machine",
            Self::Claim => "Claiming aura node",
            Self::SyncFiles => "Syncing project files",
            Self::ImportRecord => "Importing agent record",
            Self::Wake => "Waking machine",
            Self::Seal => "Sealing agent on its node",
            Self::CopyRecord => "Copying record back",
            Self::PullChanges => "Pulling changes",
            Self::Destroy => "Destroying machine",
        }
    }
}

/// Where a stage is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageState {
    /// Under way.
    Started,
    /// Finished after `ms`.
    Done {
        /// Milliseconds it took.
        ms: u64,
    },
    /// Not needed, and why.
    Skipped(String),
    /// Failed, and why.
    Failed(String),
}

/// One thing to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootEvent {
    /// The stages a move runs, in order, before the first starts. Stages
    /// that turn out to be needed besides go in before those not begun.
    Plan(Vec<BootStage>),
    /// A stage changed.
    Stage {
        /// The stage.
        stage: BootStage,
        /// Its new state.
        state: StageState,
    },
    /// Something known about the machine or the move; a later fact with
    /// the same key replaces it.
    Fact {
        /// What it is (`region`, `machine`, ...).
        key: String,
        /// Its value.
        value: String,
    },
    /// A line a command printed.
    Log {
        /// The stage the command runs in.
        stage: BootStage,
        /// Whether it came on stderr.
        stderr: bool,
        /// The line, without escapes.
        line: String,
    },
    /// How far a stage has got.
    Progress {
        /// The stage.
        stage: BootStage,
        /// Done so far.
        done: u64,
        /// Out of.
        total: u64,
    },
    /// What a running stage is waiting on now, and how long it may take;
    /// `what: None` clears it. A wait replaced without being cleared
    /// first is the same wait going on, so keeps its clock.
    Waiting {
        /// The stage.
        stage: BootStage,
        /// The call or command pending, e.g. `boxd is creating aura-x
        /// from snapshot y`.
        what: Option<String>,
        /// When it gives up, if it does.
        limit: Option<Duration>,
    },
}

/// Where boot events go. Cheap to clone; [`Self::none`] drops them.
#[derive(Debug, Clone, Default)]
pub struct BootReporter {
    tx: Option<mpsc::UnboundedSender<BootEvent>>,
}

impl BootReporter {
    /// A reporter that shows nothing.
    #[must_use]
    pub const fn none() -> Self {
        Self { tx: None }
    }

    /// A reporter and the events it sends.
    #[must_use]
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<BootEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx: Some(tx) }, rx)
    }

    /// Whether anyone listens.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.tx.as_ref().is_some_and(|tx| !tx.is_closed())
    }

    /// Send `event`.
    pub fn send(&self, event: BootEvent) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(event);
        }
    }

    /// Report the stages to come.
    pub fn plan(&self, stages: &[BootStage]) {
        self.send(BootEvent::Plan(stages.to_vec()));
    }

    /// Report a fact.
    pub fn fact(&self, key: impl Into<String>, value: impl Into<String>) {
        self.send(BootEvent::Fact {
            key: key.into(),
            value: value.into(),
        });
    }

    /// Report `stage` as not needed.
    pub fn skip(&self, stage: BootStage, reason: impl Into<String>) {
        self.send(BootEvent::Stage {
            stage,
            state: StageState::Skipped(reason.into()),
        });
    }

    /// Report how far `stage` has got.
    pub fn progress(&self, stage: BootStage, done: u64, total: u64) {
        self.send(BootEvent::Progress { stage, done, total });
    }

    /// Report a line of command output.
    pub fn log(&self, stage: BootStage, stderr: bool, line: impl Into<String>) {
        self.send(BootEvent::Log {
            stage,
            stderr,
            line: line.into(),
        });
    }

    /// Say `stage` waits on `what` now, giving up after `limit`.
    pub fn waiting(&self, stage: BootStage, what: impl Into<String>, limit: Option<Duration>) {
        self.send(BootEvent::Waiting {
            stage,
            what: Some(what.into()),
            limit,
        });
    }

    /// Say `stage` waits on nothing now.
    pub fn done_waiting(&self, stage: BootStage) {
        self.send(BootEvent::Waiting {
            stage,
            what: None,
            limit: None,
        });
    }

    /// Await `work`, showing on `stage` that it waits on `what` (giving
    /// up after `limit`) until it ends, however it ends: the line is
    /// cleared when `work` finishes, and when the wait is dropped.
    pub async fn wait<T>(
        &self,
        stage: BootStage,
        what: impl Into<String>,
        limit: Option<Duration>,
        work: impl Future<Output = T>,
    ) -> T {
        let what = what.into();
        tracing::debug!(stage = stage.key(), %what, ?limit, "boot waits");
        self.waiting(stage, what, limit);
        let _clear = ClearWait {
            reporter: self,
            stage,
            started: Instant::now(),
        };
        work.await
    }

    /// Run `work` as `stage`: started now, then done or failed with its
    /// error.
    ///
    /// # Errors
    ///
    /// `work`'s error, unchanged.
    pub async fn run<T, E: Display>(
        &self,
        stage: BootStage,
        work: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        self.send(BootEvent::Stage {
            stage,
            state: StageState::Started,
        });
        tracing::info!(stage = stage.key(), "boot stage started");
        let started = Instant::now();
        let result = work.await;
        let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let state = match &result {
            Ok(_) => {
                tracing::info!(stage = stage.key(), ms, "boot stage done");
                StageState::Done { ms }
            }
            Err(err) => {
                tracing::warn!(stage = stage.key(), ms, error = %err, "boot stage failed");
                StageState::Failed(err.to_string())
            }
        };
        self.send(BootEvent::Stage { stage, state });
        result
    }

    /// Command output of `stage`, fed chunk by chunk, reported line by
    /// line.
    #[must_use]
    pub fn output(&self, stage: BootStage) -> OutputLines {
        OutputLines {
            reporter: self.clone(),
            stage,
            stdout: LineSplitter::default(),
            stderr: LineSplitter::default(),
        }
    }
}

/// Clears a waiting line when dropped: when its wait ends or is dropped.
struct ClearWait<'a> {
    reporter: &'a BootReporter,
    stage: BootStage,
    started: Instant,
}

impl Drop for ClearWait<'_> {
    fn drop(&mut self) {
        let ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        tracing::debug!(stage = self.stage.key(), ms, "boot wait ended");
        self.reporter.done_waiting(self.stage);
    }
}

/// Splits a command's stdout and stderr into reported lines.
#[derive(Debug)]
pub struct OutputLines {
    reporter: BootReporter,
    stage: BootStage,
    stdout: LineSplitter,
    stderr: LineSplitter,
}

impl OutputLines {
    /// Take the next chunk of output.
    pub fn chunk(&mut self, stderr: bool, bytes: &[u8]) {
        let splitter = if stderr {
            &mut self.stderr
        } else {
            &mut self.stdout
        };
        for line in splitter.push(bytes) {
            self.reporter.log(self.stage, stderr, line);
        }
    }

    /// Report what is left after the command ended.
    pub fn finish(&mut self) {
        for (stderr, splitter) in [(false, &mut self.stdout), (true, &mut self.stderr)] {
            if let Some(line) = splitter.finish() {
                self.reporter.log(self.stage, stderr, line);
            }
        }
    }
}

/// Turns output chunks into clean lines: escapes removed, a line that
/// redraws itself (`\r`) kept as last drawn, long lines cut, blank lines
/// dropped.
#[derive(Debug, Default)]
pub struct LineSplitter {
    pending: Vec<u8>,
}

impl LineSplitter {
    /// The lines `bytes` completes.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(bytes);
        let mut lines = Vec::new();
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = self.pending.drain(..=end).collect();
            if let Some(line) = clean_line(&raw[..raw.len() - 1]) {
                lines.push(line);
            }
        }
        lines
    }

    /// The unfinished last line, if any.
    pub fn finish(&mut self) -> Option<String> {
        let raw = std::mem::take(&mut self.pending);
        clean_line(&raw)
    }
}

/// `raw` as shown: the text after its last carriage return, without
/// terminal escapes, trimmed and cut to [`MAX_LINE_CHARS`]. `None` when
/// nothing is left.
#[must_use]
pub fn clean_line(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let text = text.trim_end_matches('\r');
    let text = text.rsplit('\r').next().unwrap_or_default();
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                // CSI: parameters, then one final byte in @..~.
                Some('[') => {
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                // OSC: up to BEL or ST (ESC \).
                Some(']') => {
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' {
                            chars.next_if_eq(&'\\');
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    let trimmed = out.trim_end();
    if trimmed.trim().is_empty() {
        return None;
    }
    if trimmed.chars().count() > MAX_LINE_CHARS {
        let mut cut: String = trimmed.chars().take(MAX_LINE_CHARS - 1).collect();
        cut.push('…');
        return Some(cut);
    }
    Some(trimmed.to_string())
}

/// `bytes` as `1.5 GiB`, `512 MiB`, `12 KiB` or `80 B`.
#[must_use]
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut unit = 0;
    let mut whole = bytes;
    let mut rest = 0;
    while whole >= 1024 && unit < UNITS.len() - 1 {
        rest = whole % 1024;
        whole /= 1024;
        unit += 1;
    }
    if unit == 0 || whole >= 10 || rest == 0 {
        format!("{whole} {}", UNITS[unit])
    } else {
        format!("{whole}.{} {}", rest * 10 / 1024, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_split_across_chunks() {
        let mut splitter = LineSplitter::default();
        assert!(splitter.push(b"Compil").is_empty());
        assert_eq!(
            splitter.push(b"ing x\nDone\n\npart"),
            vec!["Compiling x", "Done"]
        );
        assert_eq!(splitter.finish().as_deref(), Some("part"));
        assert_eq!(splitter.finish(), None);
    }

    #[test]
    fn escapes_and_redraws_are_cleaned() {
        assert_eq!(
            clean_line(b"\x1b[1;32m   Compiling\x1b[0m aura v0.1.0").as_deref(),
            Some("   Compiling aura v0.1.0")
        );
        assert_eq!(
            clean_line(b"  10%\r  50%\r 100% done\r").as_deref(),
            Some(" 100% done")
        );
        assert_eq!(
            clean_line(b"\x1b]0;title\x07after").as_deref(),
            Some("after")
        );
        assert_eq!(clean_line(b"   \t "), None);
        let long = "x".repeat(MAX_LINE_CHARS + 50);
        let cut = clean_line(long.as_bytes()).expect("line");
        assert_eq!(cut.chars().count(), MAX_LINE_CHARS);
        assert!(cut.ends_with('…'));
    }

    #[tokio::test]
    async fn stages_report_start_then_done_or_failure() {
        let (boot, mut rx) = BootReporter::channel();
        let ok: Result<u8, String> = boot.run(BootStage::Create, async { Ok(1) }).await;
        assert_eq!(ok, Ok(1));
        let err: Result<u8, String> = boot
            .run(BootStage::Claim, async { Err("refused".to_string()) })
            .await;
        assert!(err.is_err());
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert_eq!(events.len(), 4);
        assert!(matches!(
            events[1],
            BootEvent::Stage {
                stage: BootStage::Create,
                state: StageState::Done { .. }
            }
        ));
        assert_eq!(
            events[3],
            BootEvent::Stage {
                stage: BootStage::Claim,
                state: StageState::Failed("refused".into())
            }
        );
    }

    #[test]
    fn output_is_reported_per_stream() {
        let (boot, mut rx) = BootReporter::channel();
        let mut out = boot.output(BootStage::Snapshot);
        out.chunk(false, b"one\ntw");
        out.chunk(true, b"warn\n");
        out.chunk(false, b"o");
        out.finish();
        let mut lines = Vec::new();
        while let Ok(BootEvent::Log { stderr, line, .. }) = rx.try_recv() {
            lines.push((stderr, line));
        }
        assert_eq!(
            lines,
            vec![
                (false, "one".to_string()),
                (true, "warn".to_string()),
                (false, "two".to_string())
            ]
        );
    }

    #[test]
    fn a_silent_reporter_drops_everything() {
        let boot = BootReporter::none();
        assert!(!boot.is_active());
        boot.fact("region", "nyc");
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<BootEvent>) -> Vec<BootEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    fn cleared(stage: BootStage) -> BootEvent {
        BootEvent::Waiting {
            stage,
            what: None,
            limit: None,
        }
    }

    #[tokio::test]
    async fn a_wait_shows_what_it_waits_on_then_clears() {
        let (boot, mut rx) = BootReporter::channel();
        let limit = Some(Duration::from_secs(180));
        let got = boot
            .wait(BootStage::Create, "boxd is creating aura-1", limit, async {
                7
            })
            .await;
        assert_eq!(got, 7);
        assert_eq!(
            drain(&mut rx),
            vec![
                BootEvent::Waiting {
                    stage: BootStage::Create,
                    what: Some("boxd is creating aura-1".into()),
                    limit,
                },
                cleared(BootStage::Create),
            ]
        );
    }

    #[tokio::test]
    async fn a_failed_wait_clears_too() {
        let (boot, mut rx) = BootReporter::channel();
        let got: Result<(), String> = boot
            .wait(BootStage::Claim, "claim", None, async {
                Err("refused".to_string())
            })
            .await;
        assert!(got.is_err());
        assert_eq!(drain(&mut rx).last(), Some(&cleared(BootStage::Claim)));
    }

    #[tokio::test]
    async fn a_dropped_wait_clears_too() {
        let (boot, mut rx) = BootReporter::channel();
        let timed_out = tokio::time::timeout(
            Duration::from_millis(1),
            boot.wait(BootStage::Wake, "wake", None, std::future::pending::<()>()),
        )
        .await;
        assert!(timed_out.is_err());
        let events = drain(&mut rx);
        assert_eq!(events.len(), 2);
        assert_eq!(events.last(), Some(&cleared(BootStage::Wake)));
    }

    #[tokio::test]
    async fn a_silent_reporter_still_runs_the_work() {
        let got = BootReporter::none()
            .wait(BootStage::Quota, "quota", None, async { "ran" })
            .await;
        assert_eq!(got, "ran");
    }

    #[test]
    fn connecting_is_a_stage_of_its_own() {
        assert_eq!(BootStage::Connect.key(), "connect");
        assert_eq!(BootStage::Connect.label(), "Connecting to boxd");
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_bytes(80), "80 B");
        assert_eq!(human_bytes(16 << 30), "16 GiB");
        assert_eq!(human_bytes(1536 << 20), "1.5 GiB");
        assert_eq!(human_bytes(100 << 30), "100 GiB");
    }
}
