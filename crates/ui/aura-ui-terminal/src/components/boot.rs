//! A boot console in the chat: a machine coming up, or an agent moving
//! between machines, as it happens.

use crate::events::{BootLogLine, BootStageState, BootUpdate};
use std::collections::VecDeque;
use std::time::Instant;

/// Most log lines a console keeps; older ones are dropped.
pub const MAX_BOOT_LOG_LINES: usize = 500;

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
            outcome: None,
        }
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
                for line in lines {
                    if self.log.len() == MAX_BOOT_LOG_LINES {
                        self.log.pop_front();
                        self.dropped += 1;
                    }
                    self.log.push_back(line);
                }
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
}
