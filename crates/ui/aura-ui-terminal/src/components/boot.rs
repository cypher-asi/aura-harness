//! A boot console in the chat: a machine coming up, or an agent moving
//! between machines, as it happens.

use crate::events::{BootHistory, BootLogLine, BootStageState, BootUpdate};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// Most log lines a console keeps in memory; older ones go to its spill
/// file.
pub const MAX_BOOT_LOG_LINES: usize = 500;

/// Log lines shown while a console runs, before any earlier are asked for.
pub const LIVE_BOOT_LOG_LINES: usize = 8;

/// `850 ms`, `12.4s` or `3m 05s`.
#[must_use]
pub fn boot_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{}.{}s", ms / 1000, (ms % 1000) / 100)
    } else {
        let secs = ms / 1000;
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// Numbers spill files, so two consoles never share one.
static NEXT_SPILL: AtomicU64 = AtomicU64::new(0);

/// The log lines a console dropped from memory, in a temporary file that
/// goes when the last copy of the console does.
#[derive(Debug)]
struct Spill {
    path: PathBuf,
    file: File,
    /// Where each line starts in the file
    starts: Vec<u64>,
    /// Where the file ends
    end: u64,
}

impl Spill {
    fn create() -> Option<Self> {
        let path = std::env::temp_dir().join(format!(
            "aura-boot-{}-{}.log",
            std::process::id(),
            NEXT_SPILL.fetch_add(1, Ordering::Relaxed)
        ));
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .ok()?;
        Some(Self {
            path,
            file,
            starts: Vec::new(),
            end: 0,
        })
    }

    /// Add `lines` at the end. Each is a byte saying whether it was
    /// stderr, then its text.
    fn append(&mut self, lines: &[BootLogLine]) -> std::io::Result<()> {
        let mut bytes = Vec::new();
        let mut starts = Vec::with_capacity(lines.len());
        for line in lines {
            starts.push(self.end + bytes.len() as u64);
            bytes.push(if line.stderr { b'2' } else { b'1' });
            bytes.extend_from_slice(line.text.as_bytes());
        }
        self.file.seek(SeekFrom::Start(self.end))?;
        self.file.write_all(&bytes)?;
        self.starts.extend(starts);
        self.end += bytes.len() as u64;
        Ok(())
    }

    /// Lines `from..to` of the file.
    fn read(&mut self, from: usize, to: usize) -> std::io::Result<Vec<BootLogLine>> {
        let start = self.starts[from];
        let stop = self.starts.get(to).copied().unwrap_or(self.end);
        let mut bytes = vec![0; usize::try_from(stop - start).unwrap_or(0)];
        self.file.seek(SeekFrom::Start(start))?;
        self.file.read_exact(&mut bytes)?;
        Ok((from..to)
            .map(|i| {
                let at = usize::try_from(self.starts[i] - start).unwrap_or(0);
                let next = self
                    .starts
                    .get(i + 1)
                    .map_or(bytes.len(), |s| usize::try_from(s - start).unwrap_or(0))
                    .min(bytes.len());
                let line = &bytes[at.min(next)..next];
                BootLogLine {
                    stderr: line.first() == Some(&b'2'),
                    text: String::from_utf8_lossy(line.get(1..).unwrap_or_default()).into_owned(),
                }
            })
            .collect())
    }
}

impl Drop for Spill {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A console's spill file, shared by its copies.
#[derive(Debug, Clone)]
struct SpillFile(Arc<Mutex<Spill>>);

impl SpillFile {
    fn lock(&self) -> MutexGuard<'_, Spill> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl PartialEq for SpillFile {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for SpillFile {}

/// One step of a boot console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootStep {
    /// Stable key
    pub key: String,
    /// What it does
    pub label: String,
    /// Where it is
    pub state: BootStageState,
    /// When it started, while it runs
    pub started: Option<Instant>,
    /// How far it has got
    pub progress: Option<(u64, u64)>,
}

impl BootStep {
    /// How much of it is done, from 0 to 1.
    #[must_use]
    pub fn fraction(&self) -> f64 {
        match self.state {
            BootStageState::Pending => 0.0,
            BootStageState::Running => match self.progress {
                #[allow(clippy::cast_precision_loss)]
                Some((done, total)) if total > 0 => (done as f64 / total as f64).min(1.0),
                _ => 0.0,
            },
            _ => 1.0,
        }
    }
}

/// How a console ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootOutcome {
    /// Whether it went through
    pub ok: bool,
    /// One line on how it went
    pub summary: String,
    /// How long it took
    pub elapsed_ms: u64,
}

/// A boot console's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootConsole {
    /// Console id
    pub id: String,
    /// What is happening
    pub title: String,
    /// When it began
    pub started: Instant,
    /// Steps, in the order they began
    pub steps: Vec<BootStep>,
    /// Facts, in the order first heard
    pub facts: Vec<(String, String)>,
    /// The newest log lines
    pub log: VecDeque<BootLogLine>,
    /// Log lines dropped from the front
    pub dropped: usize,
    /// The dropped lines just before `log` read back from the spill file
    /// as they were asked for: always the last `earlier.len()` dropped
    pub earlier: VecDeque<BootLogLine>,
    /// Lines of `log` shown above the newest while it runs
    revealed: usize,
    /// Where the dropped lines went, unless that could not be written
    spill: Option<SpillFile>,
    /// Set once it ended
    pub outcome: Option<BootOutcome>,
}

impl BootConsole {
    /// A console `id` headed `title`, begun now.
    #[must_use]
    pub fn new(id: &str, title: &str) -> Self {
        Self {
            id: id.to_string(),
            title: title.to_string(),
            started: Instant::now(),
            steps: Vec::new(),
            facts: Vec::new(),
            log: VecDeque::new(),
            dropped: 0,
            earlier: VecDeque::new(),
            revealed: 0,
            spill: None,
            outcome: None,
        }
    }

    /// The console `history` describes, begun at `begun_ms` (ms since the
    /// epoch; now when unknown). One that ended keeps how long it took;
    /// one still running carries on with the next update for its id.
    #[must_use]
    pub fn replay(history: &BootHistory, begun_ms: Option<u64>) -> Self {
        let mut console = Self::new(&history.id, "Cloud machine");
        let ago = begun_ms.and_then(|at| {
            let now = u64::try_from(chrono::Utc::now().timestamp_millis()).ok()?;
            now.checked_sub(at)
        });
        if let Some(started) =
            ago.and_then(|ms| Instant::now().checked_sub(Duration::from_millis(ms)))
        {
            console.started = started;
        }
        for update in history.updates.iter().cloned() {
            console.apply(update);
        }
        if let (Some(outcome), Some(ms)) = (console.outcome.as_mut(), history.elapsed_ms) {
            outcome.elapsed_ms = ms;
        }
        console
    }

    /// Lines of `log` shown: the newest few while it runs, more as
    /// earlier ones are asked for, all of them once it ended.
    #[must_use]
    pub fn log_shown(&self) -> usize {
        if self.is_running() {
            (LIVE_BOOT_LOG_LINES + self.revealed).min(self.log.len())
        } else {
            self.log.len()
        }
    }

    /// Log lines before those shown.
    #[must_use]
    pub fn hidden(&self) -> usize {
        self.dropped - self.earlier.len() + self.log.len() - self.log_shown()
    }

    /// How many of [`Self::hidden`] can still be shown: all of them,
    /// unless the spill file could not be written.
    #[must_use]
    pub fn can_show(&self) -> usize {
        let spilled = if self.spill.is_some() {
            self.dropped - self.earlier.len()
        } else {
            0
        };
        self.log.len() - self.log_shown() + spilled
    }

    /// Show up to `count` more lines before those shown: first the ones
    /// still in memory, then ones read back from the spill file.
    pub fn show_earlier(&mut self, count: usize) {
        let in_memory = (self.log.len() - self.log_shown()).min(count);
        self.revealed += in_memory;
        let rest = count - in_memory;
        let Some(spill) = self.spill.as_ref().filter(|_| rest > 0) else {
            return;
        };
        let to = self.dropped - self.earlier.len();
        let from = to.saturating_sub(rest);
        if from == to {
            return;
        }
        if let Ok(lines) = spill.lock().read(from, to) {
            for line in lines.into_iter().rev() {
                self.earlier.push_front(line);
            }
        }
    }

    /// Write `lines`, just dropped from the log, to the spill file. The
    /// file is made with the first lines dropped, and given up on if a
    /// write fails, as it would no longer hold every dropped line.
    fn spill(&mut self, lines: &[BootLogLine]) {
        if lines.is_empty() {
            return;
        }
        if self.spill.is_none() && self.dropped == lines.len() {
            self.spill = Spill::create().map(|spill| SpillFile(Arc::new(Mutex::new(spill))));
        }
        let failed = self
            .spill
            .as_ref()
            .is_some_and(|spill| spill.lock().append(lines).is_err());
        if failed {
            self.spill = None;
        }
    }

    /// The console as plain text to copy: how it went, its steps, the
    /// facts and its whole log, the lines dropped from memory read back
    /// from the spill file.
    #[must_use]
    pub fn transcript(&self) -> String {
        let mut out = self.title.clone();
        match &self.outcome {
            None => out.push_str(" (still going)\n"),
            Some(outcome) => {
                let how = if outcome.ok { "done" } else { "failed" };
                let took = boot_duration(outcome.elapsed_ms);
                let _ = writeln!(out, " ({how} after {took})\n{}", outcome.summary);
            }
        }
        if !self.steps.is_empty() {
            out.push('\n');
        }
        for step in &self.steps {
            let label = &step.label;
            let _ = match &step.state {
                BootStageState::Pending => writeln!(out, "· {label}"),
                BootStageState::Running => writeln!(out, "… {label} (running)"),
                BootStageState::Done { ms } => writeln!(out, "✓ {label}  {}", boot_duration(*ms)),
                BootStageState::Skipped(why) => writeln!(out, "– {label}  skipped: {why}"),
                BootStageState::Failed(why) => writeln!(out, "✗ {label}  {why}"),
            };
        }
        if !self.facts.is_empty() {
            out.push('\n');
        }
        let key_width = self.facts.iter().map(|(k, _)| k.chars().count()).max();
        for (key, value) in &self.facts {
            let _ = writeln!(
                out,
                "{key:<width$}  {value}",
                width = key_width.unwrap_or(0)
            );
        }
        if self.dropped + self.log.len() > 0 {
            out.push('\n');
        }
        let spilled = self.spill.as_ref().and_then(|spill| {
            let mut spill = spill.lock();
            let count = spill.starts.len();
            spill.read(0, count).ok()
        });
        match spilled {
            Some(lines) => {
                for line in lines {
                    out.push_str(&line.text);
                    out.push('\n');
                }
            }
            None if self.dropped > 0 => {
                let _ = writeln!(out, "… {} earlier lines could not be kept", self.dropped);
            }
            None => {}
        }
        for line in &self.log {
            out.push_str(&line.text);
            out.push('\n');
        }
        out
    }

    /// Whether it is still going.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.outcome.is_none()
    }

    /// The fact under `key`, if heard.
    #[must_use]
    pub fn fact(&self, key: &str) -> Option<&str> {
        self.facts
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Milliseconds since it began, or how long it took once it ended.
    #[must_use]
    pub fn elapsed_ms(&self) -> u64 {
        self.outcome.as_ref().map_or_else(
            || u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            |outcome| outcome.elapsed_ms,
        )
    }

    /// How far along it is, from 0 to 1: finished steps count whole, the
    /// running ones by their progress.
    #[must_use]
    pub fn fraction(&self) -> f64 {
        if self.steps.is_empty() {
            return 0.0;
        }
        let done: f64 = self.steps.iter().map(BootStep::fraction).sum();
        #[allow(clippy::cast_precision_loss)]
        let total = self.steps.len() as f64;
        (done / total).clamp(0.0, 1.0)
    }

    /// The step under `key`; one not heard of goes in before the steps
    /// not begun, so the order stays the order they run in.
    fn step(&mut self, key: &str, label: Option<&str>) -> &mut BootStep {
        let at = match self.steps.iter().position(|s| s.key == key) {
            Some(at) => at,
            None => {
                let at = self
                    .steps
                    .iter()
                    .position(|s| s.state == BootStageState::Pending)
                    .unwrap_or(self.steps.len());
                self.steps.insert(
                    at,
                    BootStep {
                        key: key.to_string(),
                        label: label.unwrap_or(key).to_string(),
                        state: BootStageState::Running,
                        started: Some(Instant::now()),
                        progress: None,
                    },
                );
                at
            }
        };
        &mut self.steps[at]
    }

    /// Take `update`.
    pub fn apply(&mut self, update: BootUpdate) {
        match update {
            BootUpdate::Begin { title } => self.title = title,
            BootUpdate::Plan(planned) => {
                for planned in planned {
                    if self.steps.iter().all(|s| s.key != planned.key) {
                        self.steps.push(BootStep {
                            key: planned.key,
                            label: planned.label,
                            state: BootStageState::Pending,
                            started: None,
                            progress: None,
                        });
                    }
                }
            }
            BootUpdate::Stage { key, label, state } => {
                let step = self.step(&key, Some(&label));
                step.label = label;
                step.started = match state {
                    BootStageState::Running => Some(Instant::now()),
                    _ => None,
                };
                if matches!(state, BootStageState::Running) {
                    step.progress = None;
                }
                step.state = state;
            }
            BootUpdate::Fact { key, value } => {
                match self.facts.iter_mut().find(|(k, _)| *k == key) {
                    Some(fact) => fact.1 = value,
                    None => self.facts.push((key, value)),
                }
            }
            BootUpdate::Log(lines) => {
                let mut gone = Vec::new();
                for line in lines {
                    if self.log.len() == MAX_BOOT_LOG_LINES {
                        if let Some(old) = self.log.pop_front() {
                            if !self.earlier.is_empty() {
                                self.earlier.push_back(old.clone());
                            }
                            gone.push(old);
                        }
                        self.dropped += 1;
                    }
                    self.log.push_back(line);
                }
                self.spill(&gone);
            }
            BootUpdate::Progress { key, done, total } => {
                self.step(&key, None).progress = Some((done, total));
            }
            BootUpdate::End { ok, summary } => {
                let elapsed_ms = self.elapsed_ms();
                self.steps.retain(|s| s.state != BootStageState::Pending);
                for step in &mut self.steps {
                    if step.state == BootStageState::Running {
                        step.state = if ok {
                            BootStageState::Done {
                                ms: step.started.map_or(0, |at| {
                                    u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX)
                                }),
                            }
                        } else {
                            BootStageState::Failed("stopped".into())
                        };
                        step.started = None;
                    }
                }
                self.outcome = Some(BootOutcome {
                    ok,
                    summary,
                    elapsed_ms,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::BootPlanStep;

    fn stage(key: &str, state: BootStageState) -> BootUpdate {
        BootUpdate::Stage {
            key: key.into(),
            label: format!("Doing {key}"),
            state,
        }
    }

    #[test]
    fn updates_build_the_console() {
        let mut console = BootConsole::new("b1", "Moving to the cloud");
        console.apply(stage("create", BootStageState::Running));
        console.apply(BootUpdate::Fact {
            key: "region".into(),
            value: "nyc".into(),
        });
        console.apply(BootUpdate::Fact {
            key: "region".into(),
            value: "fra".into(),
        });
        console.apply(stage("create", BootStageState::Done { ms: 1200 }));
        console.apply(BootUpdate::Progress {
            key: "import_record".into(),
            done: 2,
            total: 5,
        });
        assert_eq!(console.steps.len(), 2);
        assert_eq!(console.steps[0].state, BootStageState::Done { ms: 1200 });
        assert_eq!(console.steps[1].progress, Some((2, 5)));
        assert_eq!(console.fact("region"), Some("fra"));
        assert_eq!(console.facts.len(), 1);
        assert!(console.is_running());

        console.apply(BootUpdate::End {
            ok: false,
            summary: "claim failed".into(),
        });
        assert!(!console.is_running());
        assert_eq!(
            console.steps[1].state,
            BootStageState::Failed("stopped".into())
        );
    }

    #[test]
    fn a_plan_lists_the_steps_to_come_and_unplanned_ones_go_before_them() {
        let mut console = BootConsole::new("b1", "Bringing the agent back");
        console.apply(BootUpdate::Plan(
            ["seal", "copy_record"]
                .into_iter()
                .map(|key| BootPlanStep {
                    key: key.into(),
                    label: key.into(),
                })
                .collect(),
        ));
        console.apply(stage("wake", BootStageState::Running));
        let keys = |c: &BootConsole| c.steps.iter().map(|s| s.key.clone()).collect::<Vec<_>>();
        assert_eq!(keys(&console), ["wake", "seal", "copy_record"]);
        assert!(console.fraction().abs() < f64::EPSILON);

        console.apply(stage("wake", BootStageState::Done { ms: 900 }));
        console.apply(stage("seal", BootStageState::Running));
        console.apply(BootUpdate::Progress {
            key: "seal".into(),
            done: 1,
            total: 2,
        });
        assert!((console.fraction() - 0.5).abs() < 1e-9, "1.5 of 3 steps");

        console.apply(BootUpdate::End {
            ok: false,
            summary: "seal failed".into(),
        });
        assert_eq!(keys(&console), ["wake", "seal"], "steps never begun go");
    }

    #[test]
    fn the_log_keeps_the_newest_lines() {
        let mut console = BootConsole::new("b1", "t");
        let lines = (0..MAX_BOOT_LOG_LINES + 20)
            .map(|i| BootLogLine {
                stderr: false,
                text: i.to_string(),
            })
            .collect();
        console.apply(BootUpdate::Log(lines));
        assert_eq!(console.log.len(), MAX_BOOT_LOG_LINES);
        assert_eq!(console.dropped, 20);
        assert_eq!(console.log.front().map(|l| l.text.as_str()), Some("20"));
    }

    fn numbered(range: std::ops::Range<usize>) -> BootUpdate {
        BootUpdate::Log(
            range
                .map(|i| BootLogLine {
                    stderr: i % 7 == 0,
                    text: format!("line {i}"),
                })
                .collect(),
        )
    }

    fn texts(lines: &VecDeque<BootLogLine>) -> Vec<String> {
        lines.iter().map(|l| l.text.clone()).collect()
    }

    #[test]
    fn earlier_lines_come_from_memory_then_from_the_spill_file() {
        let mut console = BootConsole::new("b1", "t");
        console.apply(numbered(0..MAX_BOOT_LOG_LINES + 300));
        console.apply(numbered(MAX_BOOT_LOG_LINES + 300..MAX_BOOT_LOG_LINES + 350));
        assert_eq!(console.dropped, 350);
        assert_eq!(console.log_shown(), LIVE_BOOT_LOG_LINES);
        assert_eq!(
            console.hidden(),
            350 + MAX_BOOT_LOG_LINES - LIVE_BOOT_LOG_LINES
        );
        assert_eq!(console.can_show(), console.hidden());

        console.show_earlier(MAX_BOOT_LOG_LINES - LIVE_BOOT_LOG_LINES + 100);
        assert_eq!(console.log_shown(), MAX_BOOT_LOG_LINES, "memory first");
        assert_eq!(
            texts(&console.earlier),
            (250..350).map(|i| format!("line {i}")).collect::<Vec<_>>()
        );
        assert!(console.earlier[0].stderr == (250 % 7 == 0));
        assert_eq!(console.hidden(), 250);

        console.apply(numbered(850..852));
        assert_eq!(
            console.earlier.back().map(|l| l.text.as_str()),
            Some("line 351"),
            "lines dropped once earlier ones show stay shown"
        );
        console.show_earlier(1000);
        assert_eq!(console.earlier.len(), 352);
        assert_eq!(console.earlier[0].text, "line 0");
        assert_eq!(console.hidden(), 0);
        assert_eq!(console.can_show(), 0);
    }

    #[test]
    fn the_transcript_has_the_whole_log_spilled_lines_included() {
        let mut console = BootConsole::new("b1", "Moving to the cloud");
        console.apply(stage(
            "snapshot",
            BootStageState::Failed("build failed".into()),
        ));
        console.apply(BootUpdate::Fact {
            key: "machine".into(),
            value: "aura-1".into(),
        });
        console.apply(numbered(0..MAX_BOOT_LOG_LINES + 40));
        console.apply(BootUpdate::End {
            ok: false,
            summary: "Could not move the agent".into(),
        });
        let text = console.transcript();
        assert!(
            text.starts_with("Moving to the cloud (failed after "),
            "{text}"
        );
        assert!(text.contains("\nCould not move the agent\n"));
        assert!(text.contains("✗ Doing snapshot  build failed\n"));
        assert!(text.contains("machine  aura-1\n"));
        let log: Vec<&str> = text.lines().filter(|l| l.starts_with("line ")).collect();
        assert_eq!(log.len(), MAX_BOOT_LOG_LINES + 40, "every line, none twice");
        assert_eq!(log[0], "line 0");
        assert_eq!(log.last().copied(), Some("line 539"));
    }

    #[test]
    fn the_spill_file_goes_with_the_last_copy() {
        let mut console = BootConsole::new("b1", "t");
        console.apply(numbered(0..MAX_BOOT_LOG_LINES + 1));
        let path = console
            .spill
            .as_ref()
            .map(|s| s.lock().path.clone())
            .unwrap();
        assert!(path.exists());
        let copy = console.clone();
        drop(console);
        assert!(path.exists());
        drop(copy);
        assert!(!path.exists());
    }
}
