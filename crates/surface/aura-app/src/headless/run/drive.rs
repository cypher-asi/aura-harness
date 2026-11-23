//! A run's turns: its prompt's, which Gate follows up (see `follow`),
//! then one per notice of the agent's background work, until it has
//! nothing left to do or a limit stops it; and the report it ends with.

use super::session::session_id;
use super::turn::{Carry, TurnOutcome};
use super::{Headless, RunLimits};
use crate::event_loop::gate::followup::{from_harness, Before, Following};
use crate::event_loop::processes::stop_background_commands;
use crate::event_loop::AgentSession;
use crate::headless::output::{ExitReason, Output, RunReport, StreamEvent, UsageReport};
use crate::session_checkpoint::SessionUsage;
use aura_config::ReviewMode;
use aura_core_types::OutcomePayload;
use aura_gate::route::Candidate;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

/// What the turns of one run share: its limits, when it runs out of
/// time, where it streams and what interrupts it.
#[derive(Clone, Copy)]
pub(super) struct Run<'a> {
    pub(super) limits: RunLimits,
    /// When `--max-time` runs out
    pub(super) deadline: Option<Instant>,
    pub(super) output: &'a Output,
    /// Stops the run (Ctrl+C)
    pub(super) interrupt: &'a CancellationToken,
}

/// Why a run ended before its work was done, and the error it ended
/// with, if any.
pub(super) type Stop = (ExitReason, Option<String>);

/// A run's next turn: what its model is sent, and what the turn carries
/// on from the one before it.
pub(super) type Next = (String, Option<Carry>);

/// What a run has accumulated so far.
#[derive(Default)]
pub(super) struct Totals {
    /// Model calls and tokens of its turns so far
    pub(super) usage: SessionUsage,
    agent_turns: u32,
    compactions: u32,
    files_changed: Vec<String>,
    reply: String,
}

impl Totals {
    /// Count the turn that ended as `outcome`.
    fn add(&mut self, outcome: &TurnOutcome) {
        self.usage.add(&outcome.usage);
        let Some(result) = &outcome.result else {
            return;
        };
        self.compactions += result.summary_compactions;
        for change in &result.file_changes {
            if !self.files_changed.contains(&change.path) {
                self.files_changed.push(change.path.clone());
            }
        }
        if !result.total_text.trim().is_empty() {
            self.reply.clone_from(&result.total_text);
        }
    }
}

/// A run under way, on its session.
pub(super) struct Driving<'a> {
    pub(super) headless: &'a mut Headless,
    pub(super) session: &'a mut AgentSession,
    pub(super) run: Run<'a>,
    /// The run's prompt, which Gate's judge and reviewer are shown
    pub(super) task: String,
    /// The models Gate routes the run's turns among; `None` when it
    /// routes none
    pub(super) candidates: Option<Vec<Candidate>>,
    pub(super) totals: Totals,
    /// The workspace as it was when the run began, when a review may want
    /// to see everything the turns change
    pub(super) before: Option<Before>,
    /// The follow-up of the attempt at the prompt so far
    pub(super) follow: Following,
    /// Where that attempt's messages start: a turn whose model carries on
    /// with the work belongs to the attempt before it
    pub(super) attempt_start: usize,
    /// Whether the turns so far answer the run's prompt (its first turn
    /// and those that carry its work on) rather than a notice
    answering: bool,
    /// The `Outcome` recorded for the last turn answering the prompt
    pub(super) answered: Option<OutcomePayload>,
}

impl<'a> Driving<'a> {
    /// The run of `task` on `session` starting: the files its turns
    /// change are watched from here, so the judge can see what a failed
    /// turn changed; the next prompt Gate classifies is the run's own;
    /// and the workspace is recorded as it is now, when a review may want
    /// to see everything the turns change, by their commands as well as
    /// their file tools.
    pub(super) async fn start(
        headless: &'a mut Headless,
        session: &'a mut AgentSession,
        run: Run<'a>,
        task: String,
    ) -> Self {
        if let Some(files) = &session.file_checkpoints {
            files.begin_attempt(session.agent_id());
        }
        headless.criteria = None;
        headless.criteria_due = true;
        let candidates = headless.hub.gate_candidates(session.definition.as_deref());
        let settings = headless.hub.gate.settings();
        let reviews = (settings.review != ReviewMode::Off || settings.criteria != ReviewMode::Off)
            && settings.judge
            && candidates
                .as_ref()
                .is_some_and(|candidates| !candidates.is_empty());
        let before = if reviews {
            Before::of_workspace(session.kernel.agent_workspace()).await
        } else {
            None
        };
        Self {
            headless,
            session,
            run,
            task,
            candidates,
            totals: Totals::default(),
            before,
            follow: Following::default(),
            attempt_start: 0,
            answering: true,
            answered: None,
        }
    }
}

impl Driving<'_> {
    /// Run the turns of the run's prompt, then of the notices of its
    /// background work, until it has nothing left to do; returns what
    /// stopped it, if anything did.
    pub(super) async fn turns(&mut self) -> Option<Stop> {
        let mut next = Some((self.task.clone(), None));
        while let Some((text, carry)) = next {
            let outcome = self.turn(text, carry).await;
            next = match self.after(outcome).await {
                Ok(next) => next,
                Err(stop) => return Some(stop),
            };
        }
        None
    }

    /// Run a turn sending `text`, carrying on as `carry` says, and count
    /// it.
    async fn turn(&mut self, text: String, carry: Option<Carry>) -> TurnOutcome {
        self.totals.agent_turns += 1;
        let nudged = matches!(carry, Some(Carry::Nudge(..)));
        let outcome = self.run_turn(text, carry).await;
        if !nudged {
            self.attempt_start = outcome.turn_start;
        }
        self.totals.add(&outcome);
        outcome
    }

    /// What follows the turn that ended as `outcome`: the turn that
    /// carries its work on, when Gate has one do so, else the turn of
    /// the next notice; `Ok(None)` when there is none, and `Err` when the
    /// run stops.
    async fn after(&mut self, mut outcome: TurnOutcome) -> Result<Option<Next>, Stop> {
        if !self.answering {
            self.settle(&outcome).await;
        } else if let Some(next) = self.follow(&mut outcome).await {
            return Ok(Some(next));
        }
        if let Some(stop) = outcome.stop {
            return Err(stop);
        }
        self.answering = false;
        let next = if outcome.unseen.is_empty() {
            let kernel = &self.session.kernel;
            let waited = self.headless.next_notice(kernel, self.run).await;
            match waited.map_err(|reason| (reason, None))? {
                Some(notice) => notice,
                None => return Ok(None),
            }
        } else {
            outcome.unseen.join("\n\n")
        };
        self.run.output.event(&StreamEvent::Notice { text: &next });
        Ok(Some((from_harness(&next), None)))
    }

    /// End the run, stopped as `stop` says when something stopped it: the
    /// agent's background commands are stopped, the files the turns
    /// change no longer watched, and the run's report made; it began at
    /// `started`, `started_ms` in Unix milliseconds. Also returns the
    /// `Outcome` recorded for the last turn answering its prompt.
    pub(super) fn end(
        self,
        stop: Option<Stop>,
        started: Instant,
        started_ms: u64,
    ) -> (RunReport, Option<OutcomePayload>) {
        let agent = self.session.agent_id();
        stop_background_commands(self.headless.background.as_ref());
        if let Some(files) = &self.session.file_checkpoints {
            let _ = files.take_attempt(agent);
        }
        let (exit_reason, error) = stop.unwrap_or((ExitReason::Success, None));
        let totals = self.totals;
        let usage = UsageReport::from(&totals.usage);
        let gate_cost_usd = self.headless.hub.gate_cost_usd(agent, started_ms);
        let report = RunReport {
            kind: "result",
            exit_reason,
            is_error: exit_reason != ExitReason::Success,
            result: totals.reply,
            session_id: session_id(agent, self.session.started_ms),
            model: self.headless.model.clone(),
            num_turns: totals.usage.calls,
            agent_turns: totals.agent_turns,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            total_cost_usd: [usage.cost_usd, gate_cost_usd]
                .into_iter()
                .flatten()
                .reduce(|work, gate| work + gate),
            gate_cost_usd,
            usage,
            compactions: totals.compactions,
            todos: self.session.todos.clone(),
            files_changed: totals.files_changed,
            error,
        };
        (report, self.answered)
    }
}
