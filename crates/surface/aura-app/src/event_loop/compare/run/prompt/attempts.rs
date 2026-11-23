//! The attempts at a prompt's turn: the first runs where the run's arm
//! routed the prompt, and on a Gate run each attempt that ended goes to
//! Gate's follow-up and, when it failed, to Gate's judge and on to a
//! stronger model, until an attempt stands.

use super::route::{effort_of, Routed};
use super::stronger::Failure;
use super::Turned;
use crate::event_loop::agent_events::ForwarderState;
use crate::event_loop::agents::AgentSession;
use crate::event_loop::compare::run::context::{elapsed_ms, STOPPED};
use crate::event_loop::compare::run::live::LiveRoute;
use crate::event_loop::compare::run::turn::RunParts;
use crate::event_loop::gate::followup::{ended_signal, failure_text, Before, Followed, Following};
use crate::event_loop::handlers::{handle_agent_success, run_agent_turn};
use crate::event_loop::steering::Steering;
use crate::event_loop::trace;
use aura_agent::{AgentError, AgentLoop, AgentLoopConfig, AgentLoopResult};
use aura_core_types::{Difficulty, Hash, OutcomePayload, OutcomeSignal, PromptCategory, RunTurn};
use aura_gate::attempts::Terms;
use aura_gate::evidence::outcome::calls_for_stronger;
use aura_gate::evidence::Evidence;
use aura_gate::judge::criteria::Criteria;
use aura_gate::live;
use aura_gate::route::Decision;
use aura_model::{Message, ToolDefinition};
use aura_ui_terminal::UiCommand;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// How an attempt's run ended: its result, and what its events told.
type AttemptRun = (Result<AgentLoopResult, AgentError>, ForwarderState);

/// A prompt of a run as it was routed, for the attempts at its turn.
pub(super) struct Asked<'a> {
    /// What the prompt says
    pub text: &'a str,
    pub category: PromptCategory,
    /// How hard the classifier judged it
    pub difficulty: Option<Difficulty>,
    /// Whether Gate had the turn led
    pub led: bool,
    /// What a Gate run's work is held to, written down before it started
    pub criteria: Option<Arc<Criteria>>,
    /// What the copy held before the prompt, for a review of a Gate run's
    /// work
    pub before: Option<Before>,
    /// The model the prompt went to, and the effort its requests were
    /// routed at
    pub model: String,
    pub effort: Option<String>,
    /// The prompt as recorded; `None` when it could not be recorded
    pub prompt_tx: Option<Hash>,
    /// When its turn started, and the same in Unix milliseconds
    pub started: Instant,
    pub started_ms: u64,
}

/// The attempt at a prompt's turn that runs, or last ran.
pub(super) struct Current {
    /// The model it runs on
    pub model: String,
    /// The recorded prompt it answers: the user's, or a stronger model's
    /// takeover; `None` when it could not be recorded
    pub prompt_tx: Option<Hash>,
    /// Where its own messages start in the session
    pub turn_start: usize,
    /// When it started, and the same in Unix milliseconds
    pub started: Instant,
    pub started_ms: u64,
    /// What its runs before its last one cost
    pub cost_usd: f64,
}

/// How a prompt's turn ended.
pub(super) struct Ended {
    pub signal: OutcomeSignal,
    pub reply: String,
    pub error: Option<String>,
    /// Its last attempt's outcome; `None` when the prompt that attempt
    /// answers could not be recorded
    pub outcome: Option<OutcomePayload>,
    /// Whether that outcome is logged and recorded already
    pub recorded: bool,
}

/// A prompt's turn while its attempts run.
pub(super) struct Attempting<'a, 'p> {
    pub parts: &'a RunParts<'p>,
    pub session: &'a mut AgentSession,
    pub asked: Asked<'a>,
    /// What the next attempt runs with
    pub config: AgentLoopConfig,
    pub tools: Vec<ToolDefinition>,
    /// Where the run works
    pub workdir: PathBuf,
    pub current: Current,
    pub follow: Following,
    /// What the last attempt left, for Gate's attempt log
    pub left: Option<Evidence>,
    /// Whether the model's provider cut the last attempt short
    pub cut_short: bool,
    /// How often the turn went on to a stronger model, was asked to check
    /// its change, and was sent back with what is wrong
    pub escalations: u32,
    pub nudges: u32,
    pub sent_back: u32,
}

impl<'a, 'p> Attempting<'a, 'p> {
    /// The attempts at the prompt `routed` says, on `session`: the prompt
    /// goes into the conversation with what its work is held to, and a
    /// Gate run's attempt is watched for the files it changes, so a judge
    /// can see what a failed one changed.
    pub(super) fn start(
        parts: &'a RunParts<'p>,
        session: &'a mut AgentSession,
        routed: Routed<'a>,
    ) -> Self {
        let Routed {
            asked,
            config,
            tools,
        } = routed;
        show_routed(parts, &asked, &asked.model);
        let current = Current {
            model: asked.model.clone(),
            prompt_tx: asked.prompt_tx,
            turn_start: session.messages.len(),
            started: asked.started,
            started_ms: asked.started_ms,
            cost_usd: 0.0,
        };
        let note = live::worker_note_for(asked.criteria.as_deref()).unwrap_or_default();
        let text = format!("{}{note}", asked.text);
        session.messages.push(Message::user(text));
        if parts.gate_arm() {
            if let Some(files) = &session.file_checkpoints {
                files.begin_attempt(session.agent_id());
            }
        }
        let workdir = session.kernel.agent_workspace();
        Self {
            parts,
            session,
            asked,
            config,
            tools,
            workdir,
            current,
            follow: Following::default(),
            left: None,
            cut_short: false,
            escalations: 0,
            nudges: 0,
            sent_back: 0,
        }
    }

    /// Run attempts until one stands; returns how the turn ended.
    pub(super) async fn run(&mut self) -> Ended {
        loop {
            let ran = self.attempt().await;
            if let ControlFlow::Break(ended) = self.goes_on(ran).await {
                return ended;
            }
        }
    }

    /// Run an attempt on the model the turn is on, and end it in the run's
    /// trace, so a nudge, a send-back or a stronger model's takeover shows
    /// where the attempt before it stopped.
    async fn attempt(&mut self) -> AttemptRun {
        let parts = self.parts;
        let agent_loop = AgentLoop::new(self.config.clone());
        let steering = Steering::new(&parts.cancel);
        let ran = run_agent_turn(
            self.session,
            &agent_loop,
            self.tools.clone(),
            &parts.ui,
            &parts.cancel,
            &steering,
            parts.ctx.hub.long_run,
            parts.recorder.clone(),
        )
        .await;
        steering.finish(&[]);
        if let Some(recorder) = &parts.recorder {
            trace::end_turn(recorder, &ran.0, parts.cancel.is_cancelled());
        }
        ran
    }

    /// Whether another attempt follows the one that ended as `ran`, set
    /// up when one does; else how the turn ended.
    async fn goes_on(&mut self, (result, forwarded): AttemptRun) -> ControlFlow<Ended> {
        let gate_arm = self.parts.gate_arm();
        let stopped = self.parts.cancel.is_cancelled() || forwarded.credentials_rejected;
        let turn_start = self.current.turn_start;
        let mut signal = if stopped {
            Some(OutcomeSignal::Cancelled)
        } else {
            ended_signal(&result, turn_start)
        };
        self.cut_short = signal == Some(OutcomeSignal::Unavailable);
        let mut followed = Followed::default();
        if let (true, false, Ok(partial)) = (gate_arm, stopped, &result) {
            followed = self.follow_up(partial, signal).await;
            signal = followed.signal;
        }
        let signal = signal.unwrap_or(OutcomeSignal::Unavailable);
        let reason = (followed.harness.clone())
            .or_else(|| failure_text(&result, turn_start))
            .unwrap_or_default();
        let survived = result.is_ok();
        self.left =
            (result.as_ref().ok()).map(|result| self.follow.steps.evidence(result, turn_start));
        let diff = self.diff(&result, signal).await;
        let (reply, error) = self.answer(result, &forwarded).await;
        if let Some(carry_on) = followed.carry_on {
            self.carry_on(carry_on, &forwarded).await;
            return ControlFlow::Continue(());
        }
        let outcome = self.outcome(signal, forwarded.usage.cost_usd);
        if !(gate_arm && survived && calls_for_stronger(signal)) {
            return ControlFlow::Break(Ended {
                signal,
                reply,
                error,
                outcome,
                recorded: false,
            });
        }
        let failure = Failure {
            signal,
            outcome,
            reply,
            error,
            diff,
            reason,
            harness: followed.harness,
            unchecked: followed.wrong,
        };
        self.escalate(failure).await
    }

    /// What a judge of a Gate run's attempt that ended in `result` is
    /// shown of its change, when `signal` calls for a stronger model.
    async fn diff(
        &mut self,
        result: &Result<AgentLoopResult, AgentError>,
        signal: OutcomeSignal,
    ) -> String {
        match result {
            Ok(partial) if self.parts.gate_arm() && calls_for_stronger(signal) => {
                self.follow.steps.diff(self.session, Some(partial)).await
            }
            _ => String::new(),
        }
    }

    /// The attempt's reply, and the error it ended on: its `result` kept
    /// in the session (see [`handle_agent_success`]), or its failure
    /// shown.
    async fn answer(
        &mut self,
        result: Result<AgentLoopResult, AgentError>,
        forwarded: &ForwarderState,
    ) -> (String, Option<String>) {
        let ui = &self.parts.ui;
        match result {
            Ok(result) => {
                let text = result.total_text.clone();
                let llm_error = result.llm_error.clone().filter(|_| text.trim().is_empty());
                handle_agent_success(self.session, result, forwarded, ui, &[]).await;
                (text, llm_error)
            }
            Err(e) => {
                ui.send(UiCommand::ShowWarning(e.to_string())).await;
                ui.send(UiCommand::Complete).await;
                (String::new(), Some(e.to_string()))
            }
        }
    }

    /// The outcome of the attempt that ended with `signal`, its last run
    /// having cost `cost_usd`; `None` when the prompt it answers could not
    /// be recorded.
    fn outcome(&self, signal: OutcomeSignal, cost_usd: f64) -> Option<OutcomePayload> {
        let current = &self.current;
        let hub = &self.parts.ctx.hub;
        let agent_id = self.session.agent_id();
        current.prompt_tx.map(|prompt_tx| OutcomePayload {
            prompt_tx,
            category: self.asked.category,
            model: current.model.clone(),
            success: signal.success(),
            signal,
            cost_usd: current.cost_usd + cost_usd,
            subtask_cost_usd: hub.subtask_cost_usd(agent_id, current.started_ms),
            latency_ms: elapsed_ms(current.started),
        })
    }

    /// What the turn's attempts run under, for Gate's attempt log.
    pub(super) fn terms(&self) -> Terms {
        Terms {
            difficulty: self.asked.difficulty,
            led: Some(self.asked.led),
            routed: self.parts.gate_arm(),
        }
    }

    /// Point the turn's next attempt at `next`'s model and effort.
    pub(super) fn point(&mut self, next: &Decision) {
        let hub = &self.parts.ctx.hub;
        hub.model_limits().route(&mut self.config, next);
        hub.gate.note_model(self.session.agent_id(), &next.model.id);
        self.current.model.clone_from(&next.model.id);
        show_routed(self.parts, &self.asked, &self.current.model);
    }

    /// The turn as its attempts left it once they ended as `ended`, its
    /// last attempt's outcome logged and recorded unless it already was.
    pub(super) async fn end(self, ended: Ended) -> Turned {
        let Ended {
            signal,
            reply,
            mut error,
            outcome,
            recorded,
        } = ended;
        let (parts, agent_id) = (self.parts, self.session.agent_id());
        if let Some(files) = (self.session.file_checkpoints.as_ref()).filter(|_| parts.gate_arm()) {
            let _ = files.take_attempt(agent_id);
        }
        if parts.cancel.is_cancelled() {
            error = Some(STOPPED.to_string());
        }
        if let (false, Some(outcome)) = (recorded, &outcome) {
            let terms = self.terms();
            parts
                .ctx
                .hub
                .gate
                .log_attempt(agent_id, outcome, terms, self.left);
            live::record(&self.session.kernel, agent_id, outcome).await;
        }
        let asked = self.asked;
        let effort = effort_of(&self.config);
        let turn = RunTurn {
            category: Some(asked.category),
            difficulty: asked.difficulty,
            led: Some(asked.led),
            escalations: self.escalations,
            first_model: (self.current.model != asked.model).then_some(asked.model),
            first_effort: asked.effort.filter(|first| effort.as_ref() != Some(first)),
            effort,
            nudges: self.nudges,
            sent_back: self.sent_back,
            subtask_cost_usd: parts.ctx.hub.subtask_cost_usd(agent_id, asked.started_ms),
            ..RunTurn::new(self.current.model, 0.0, elapsed_ms(asked.started), signal)
        };
        Turned {
            prompt_tx: asked.prompt_tx,
            reply,
            error,
            ended: outcome,
            cut_short: self.cut_short,
            turn,
        }
    }
}

/// `parts`' run is on `asked`'s turn, which runs on `model`, as the BENCH
/// tab tells.
fn show_routed(parts: &RunParts<'_>, asked: &Asked<'_>, model: &str) {
    let route = LiveRoute {
        model: model.to_string(),
        category: asked.category,
        difficulty: asked.difficulty,
    };
    parts.ctx.routed(&parts.run.name, route);
}
