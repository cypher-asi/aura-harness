//! [`BootConsole::apply`]: taking an update, one handler per kind.

use super::spill::{Spill, SpillFile};
use super::{BootConsole, BootOutcome, BootStep, BootWait, MAX_BOOT_LOG_LINES};
use aura_ui::events::{BootLogLine, BootPlanStep, BootStageState, BootUpdate};
use std::sync::{Arc, Mutex};
use std::time::Instant;

impl BootConsole {
    /// Take `update`.
    pub fn apply(&mut self, update: BootUpdate) {
        self.last_update = Instant::now();
        match update {
            BootUpdate::Begin { title } => self.title = title,
            BootUpdate::Plan(planned) => self.plan(planned),
            BootUpdate::Stage { key, label, state } => self.stage(&key, label, state),
            BootUpdate::Fact { key, value } => self.set_fact(key, value),
            BootUpdate::Log(lines) => self.push_log(lines),
            BootUpdate::Progress { key, done, total } => {
                self.step(&key, None).progress = Some((done, total));
            }
            BootUpdate::Waiting {
                key,
                what,
                limit_ms,
            } => self.wait(&key, what, limit_ms),
            BootUpdate::End { ok, summary } => self.end(ok, summary),
        }
    }

    /// Add the `planned` steps not heard of yet, not begun.
    fn plan(&mut self, planned: Vec<BootPlanStep>) {
        for planned in planned {
            if self.steps.iter().all(|s| s.key != planned.key) {
                self.steps.push(BootStep {
                    key: planned.key,
                    label: planned.label,
                    state: BootStageState::Pending,
                    started: None,
                    progress: None,
                    waiting: None,
                });
            }
        }
    }

    /// Step `key`, labelled `label`, is now in `state`.
    fn stage(&mut self, key: &str, label: String, state: BootStageState) {
        let step = self.step(key, Some(&label));
        step.label = label;
        step.started = match state {
            BootStageState::Running => Some(Instant::now()),
            _ => None,
        };
        if matches!(state, BootStageState::Running) {
            step.progress = None;
        }
        // A step that starts again or ends waits on nothing yet.
        step.waiting = None;
        step.state = state;
    }

    /// Set the fact under `key`, keeping its place if heard before.
    fn set_fact(&mut self, key: String, value: String) {
        match self.facts.iter_mut().find(|(k, _)| *k == key) {
            Some(fact) => fact.1 = value,
            None => self.facts.push((key, value)),
        }
    }

    /// Add `lines` to the log, dropping the oldest past
    /// [`MAX_BOOT_LOG_LINES`] into the spill file.
    fn push_log(&mut self, lines: Vec<BootLogLine>) {
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

    /// Step `key` now waits on `what`, or on nothing when `None`.
    fn wait(&mut self, key: &str, what: Option<String>, limit_ms: Option<u64>) {
        match what {
            Some(what) => {
                let step = self.step(key, None);
                let since = step
                    .waiting
                    .as_ref()
                    .map_or_else(Instant::now, |wait| wait.since);
                step.waiting = Some(BootWait {
                    what,
                    since,
                    limit_ms,
                });
            }
            // A clear never adds a step.
            None => {
                if let Some(step) = self.steps.iter_mut().find(|s| s.key == key) {
                    step.waiting = None;
                }
            }
        }
    }

    /// The console ended, `ok` or not: steps never begun go, and running
    /// ones finish or fail.
    fn end(&mut self, ok: bool, summary: String) {
        let elapsed_ms = self.elapsed_ms();
        self.steps.retain(|s| s.state != BootStageState::Pending);
        for step in &mut self.steps {
            step.waiting = None;
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
                        waiting: None,
                    },
                );
                at
            }
        };
        &mut self.steps[at]
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
}
