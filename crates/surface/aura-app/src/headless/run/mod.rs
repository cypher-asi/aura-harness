//! One `aura -p` run on the terminal's session machinery: the agent hub
//! and kernel factory open the session, turns run through the same
//! [`drive_turn`] as the terminal's, and the session is recorded and
//! checkpointed after every turn so `--resume` / `--continue` and the
//! terminal's `/resume` can pick it up.
//!
//! - `session`: the session a run continues, and the line that
//!   announces it
//! - `drive`: a run's turns, from its prompt to the last notice of its
//!   background work, and the report it ends with
//! - `turn`: one agent turn, from its prompt to how it ended
//! - `route`: Gate's choice of model for a turn
//! - `follow`: what Gate does once a turn answering the run's prompt
//!   ends: its follow-up, its record, its judge and the move to a
//!   stronger model
//! - `notices`: the notices of the agent's background work
//!
//! [`drive_turn`]: crate::event_loop::handlers::drive_turn

#![deny(clippy::too_many_lines)]

mod drive;
mod follow;
mod notices;
mod route;
mod session;
#[cfg(test)]
mod tests;
mod turn;

use super::output::{Output, RunReport};
use super::tools::ToolFilter;
use crate::event_loop::notices::Notice;
use crate::event_loop::{AgentHub, AgentSession};
use aura_agent_trace::RunRecorder;
use aura_core_modes::AgentMode;
use aura_core_types::time::now_ms;
use aura_core_types::{OutcomePayload, Transaction};
use aura_exec_tools::BackgroundProcessHook;
use aura_gate::judge::criteria::Criteria;
use aura_gate::live::Judged;
use aura_model::model_catalog::route_label;
use aura_model::ThinkingEffort;
use drive::{Driving, Run};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Which session a run continues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionChoice {
    /// A fresh session of the headless agent.
    New,
    /// The headless agent's current session.
    Continue,
    /// The saved session with this id.
    Resume(String),
}

/// Run-wide limits.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RunLimits {
    pub max_turns: Option<u32>,
    pub max_time: Option<Duration>,
    pub max_cost_usd: Option<f64>,
    /// Iteration cap of each turn, instead of the agent's own
    pub max_iterations: Option<usize>,
}

pub(crate) struct Request {
    pub prompt: String,
    pub session: SessionChoice,
    pub limits: RunLimits,
}

/// The parts of a headless run that outlive one request.
pub struct Headless {
    pub(super) hub: Arc<AgentHub>,
    /// Every agent's background commands
    pub(super) background: Arc<dyn BackgroundProcessHook>,
    /// Background commands that ended
    pub(super) completions: mpsc::Receiver<Transaction>,
    /// Replies from `task` spawns and background agents
    pub(super) notices: mpsc::UnboundedReceiver<Notice>,
    /// Model the turns run on, for the report
    pub(super) model: String,
    /// The effort Gate chose for the turn, while that is Gate's to choose
    pub(super) effort: Option<ThinkingEffort>,
    /// The effort the last turn's requests were held at, for a turn that
    /// carries on from it
    pub(super) held_effort: Option<ThinkingEffort>,
    pub(super) mode: AgentMode,
    pub(super) workspace_root: PathBuf,
    pub(super) tool_filter: ToolFilter,
    /// Longest wait for the agent's background work (commands, spawns)
    /// once it has nothing else to do
    pub(super) job_wait: Duration,
    /// Run trace the turns are recorded to, for an eval task's run
    pub(super) trace: Option<Arc<RunRecorder>>,
    /// Problems found while setting up that change what the run does,
    /// such as a plan's model list that would not load
    pub(super) notes: Vec<String>,
    /// What Gate's judge found on the turns that failed, oldest first;
    /// whoever reports on the run takes them
    pub(super) judged: Vec<Judged>,
    /// What the work on the run's prompt is checked against, written
    /// down before it started
    pub(super) criteria: Option<Arc<Criteria>>,
    /// Whether the next prompt Gate classifies is the run's own, whose
    /// criteria are written, rather than a notice
    pub(super) criteria_due: bool,
}

impl Headless {
    /// What a caller should show before the first turn: the problems
    /// found while setting up, then, when Gate routes the turns, the
    /// model that classifies prompts and the models it routes among,
    /// cheapest first.
    #[must_use]
    pub fn setup_lines(&self) -> Vec<String> {
        let mut lines = self.notes.clone();
        if let Some(candidates) = self.hub.gate_candidates(None) {
            let classifier = self.hub.classifier();
            lines.push(format!(
                "gate: prompts classified by {}",
                if classifier.is_empty() {
                    "nothing".to_string()
                } else {
                    route_label(&classifier)
                }
            ));
            let models: Vec<String> = candidates.iter().map(|c| route_label(&c.id)).collect();
            lines.push(format!("gate: routing among {}", models.join(", ")));
        }
        lines
    }

    /// Run `request` to the end, streaming to `output`. `interrupt` stops
    /// the run (Ctrl+C).
    pub(super) async fn execute(
        &mut self,
        request: Request,
        output: &Output,
        interrupt: &CancellationToken,
    ) -> RunReport {
        let started = Instant::now();
        let (mut session, resumed) = match self.open(&request.session).await {
            Ok(opened) => opened,
            Err(problem) => return RunReport::failed(problem, self.model.clone()),
        };
        self.announce(output, &session, resumed);
        let (report, _) = self
            .drive(
                &mut session,
                request.prompt,
                request.limits,
                started,
                output,
                interrupt,
            )
            .await;
        report
    }

    /// Run `prompt` on `session`, then the notices of the agent's
    /// background work, until it has nothing left to do or a limit stops
    /// it; `started` is when the run began. When Gate routes `prompt`'s
    /// turn, Gate follows it up first: a turn that ran no check is asked
    /// for one, and work that is hard or shows a sign of failure is
    /// reviewed before it stands. Each routed turn's `Outcome` and
    /// attempt line are written once, after its follow-up, unless its
    /// model carries on with the work. When it ends in a model error,
    /// with its last check or the harness's failing, or with its
    /// unchecked work judged unfinished, Gate's judge says whether the
    /// model was at fault. If it was, or nothing judged the turn, it goes
    /// on in the same conversation on the next stronger model, tier by
    /// tier. Also returns the `Outcome` recorded for the last of those
    /// turns, when Gate routed them.
    pub(super) async fn drive(
        &mut self,
        session: &mut AgentSession,
        prompt: String,
        limits: RunLimits,
        started: Instant,
        output: &Output,
        interrupt: &CancellationToken,
    ) -> (RunReport, Option<OutcomePayload>) {
        let run = Run {
            limits,
            deadline: limits.max_time.map(|limit| started + limit),
            output,
            interrupt,
        };
        let started_ms = now_ms();
        let mut driving = Driving::start(self, session, run, prompt).await;
        let stop = driving.turns().await;
        driving.end(stop, started, started_ms)
    }
}
