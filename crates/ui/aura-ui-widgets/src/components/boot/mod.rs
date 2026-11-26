//! A boot console in the chat: a machine coming up, or an agent moving
//! between machines, as it happens.

use aura_ui::events::{BootHistory, BootLogLine, BootStageState};
use spill::SpillFile;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

mod spill;
mod transcript;
mod update;

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

/// How long `wait` has gone on, against its limit: `42.0s of 3m 00s`,
/// or `42.0s` without one.
#[must_use]
pub fn wait_clock(wait: &BootWait) -> String {
    let waited = boot_duration(wait.waited_ms());
    match wait.limit_ms {
        Some(limit) => format!("{waited} of {}", boot_duration(limit)),
        None => waited,
    }
}

/// What a running step waits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootWait {
    /// The call or command pending
    pub what: String,
    /// When the wait began: kept while it is only reworded
    pub since: Instant,
    /// When it gives up, ms, if it does
    pub limit_ms: Option<u64>,
}

impl BootWait {
    /// Milliseconds it has waited.
    #[must_use]
    pub fn waited_ms(&self) -> u64 {
        u64::try_from(self.since.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

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
    /// What it waits on now, while it runs
    pub waiting: Option<BootWait>,
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
    /// When it last heard anything
    pub last_update: Instant,
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
            last_update: Instant::now(),
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

    /// Milliseconds since it last heard anything.
    #[must_use]
    pub fn quiet_ms(&self) -> u64 {
        u64::try_from(self.last_update.elapsed().as_millis()).unwrap_or(u64::MAX)
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
}

#[cfg(test)]
mod tests;
