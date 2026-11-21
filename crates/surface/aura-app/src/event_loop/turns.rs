//! Agent work running on its own task.
//!
//! Every turn (and every command typed in a chat) runs on a spawned task
//! that owns the agent's session until it is done, so agents work in
//! parallel and the event loop keeps handling the UI meanwhile. Each agent
//! still does one thing at a time: its record has a single writer.

use super::agent_events::AgentUi;
use super::agent_events::ForwarderState;
use super::agents::AgentSession;
use super::attachments::{expand_mentions_in, mentioned_paths};
use super::gate_followup::{
    check_review, check_signal, harness_check, joined, judge_unchecked, may_review, nudge_prompt,
    repair_message, repair_notice, sign_repair_message, sign_repair_notice, At, Before, FollowUp,
    Reviewing, Unchecked,
};
use super::gate_live::GateLive;
use super::gate_log;
use super::gate_record;
use super::handlers::{handle_agent_success, record_response, run_agent_turn, system_message};
use super::run_command::run_user_command;
use super::steering::{SteerSource, Steering};
use super::subagents::{AgentHub, LeadProfile};
use super::task_check::takeover_prompt;
use super::tasks::{request_state, summary_of};
use crate::long_run::LongRunSettings;
use crate::model_selection::ModelLimits;
use aura_agent::{AgentLoop, CompactHistoryError};
use aura_agent_gate::attempt_log::Terms;
use aura_agent_gate::checks::{last_check, last_check_seen};
use aura_agent_gate::followup::Step;
use aura_agent_gate::outcome::{calls_for_stronger, turn_signal};
use aura_agent_gate::turn_judge::Failed;
use aura_agent_gate::{Candidate, Decision, Work as GateWork};
use aura_agent_trace::RunRecorder;
use aura_core_types::{
    AgentId, Difficulty, Hash, OutcomePayload, OutcomeSignal, PromptCategory, PromptPayload,
    RecordEntry, Route, RouteLevel, RouteReason, Transaction,
};
use aura_model::{ContentBlock, ImageSource, Message, Role, ToolDefinition};
use aura_ui_terminal::events::ImageAttachment;
use aura_ui_terminal::UiCommand;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// A message for the model, with what the user attached to it.
pub(super) struct Prompt {
    /// The message as typed; the chat and the record keep this
    pub text: String,
    /// Whether `@path` words in `text` attach files. Only text the user
    /// typed has mentions; a harness notice may quote `@` words verbatim.
    pub mentions: bool,
    /// Images pasted into the message
    pub images: Vec<ImageAttachment>,
}

impl Prompt {
    /// A message typed by the user, with the images pasted into it.
    pub(super) const fn typed(text: String, images: Vec<ImageAttachment>) -> Self {
        Self {
            text,
            mentions: true,
            images,
        }
    }

    /// A message written by the harness, given to the model as is.
    pub(super) const fn plain(text: String) -> Self {
        Self {
            text,
            mentions: false,
            images: Vec::new(),
        }
    }
}

/// Something the user asked an agent to do.
pub(super) enum Work {
    /// A message for the model. `shown` is whether the chat already shows
    /// it.
    Prompt { prompt: Prompt, shown: bool },
    /// A shell command typed in the chat, run in a terminal `cols` wide.
    Command { command: String, cols: u16 },
    /// Summarize the older history now, keeping `focus` in particular.
    Compact { focus: Option<String> },
    /// Implement the plan at `path` (`/build`), or the agent's latest plan.
    Build { path: Option<String> },
}

/// Work ready to run.
pub(super) enum Task {
    /// A model turn answering `prompt`, taking further messages through
    /// `steering` while it runs.
    Turn {
        prompt: Prompt,
        agent_loop: Box<AgentLoop>,
        tools: Vec<ToolDefinition>,
        steering: Steering,
        limits: LongRunSettings,
        recorder: Option<Arc<RunRecorder>>,
        /// What Gate routes the turn among, when it routes it
        gate: Option<Box<GateTurn>>,
        /// How Gate records the turn's prompt
        routing: Box<Routing>,
    },
    /// A typed shell command.
    Command { command: String, cols: u16 },
    /// A summary of the older history, written by `agent_loop`'s model.
    Compact {
        agent_loop: Box<AgentLoop>,
        focus: Option<String>,
    },
}

/// What Gate routes a turn among once its prompt is classified, and how
/// to point an agent loop at its choice.
pub(super) struct GateTurn {
    /// Models the turn may go to
    pub candidates: Vec<Candidate>,
    pub limits: ModelLimits,
    /// What Gate has learned
    pub live: Arc<GateLive>,
    /// The model selector and status line as they stand, for showing the
    /// turn's route
    pub models: UiCommand,
    /// How the turn leads when Gate has it led; `None` when the agent
    /// never leads
    pub lead: Option<LeadProfile>,
    /// For what the turn's subtasks cost
    pub hub: Arc<AgentHub>,
}

impl GateTurn {
    /// Gate's choice for a turn asking for `category`, of `difficulty`,
    /// after the models in `refused` turned the user's sign-in down; it
    /// explores only on the first choice.
    fn decide(
        &self,
        category: PromptCategory,
        difficulty: Option<Difficulty>,
        refused: &[String],
    ) -> Option<Decision> {
        let work = GateWork::turn(category).of(difficulty);
        if refused.is_empty() {
            return self.live.decide(&work, &self.candidates);
        }
        let left: Vec<Candidate> = self
            .candidates
            .iter()
            .filter(|c| !refused.contains(&c.id))
            .cloned()
            .collect();
        self.live.redecide(&work, &left)
    }

    /// Point `agent_loop`, running `agent`'s turn, at `decision`'s model.
    fn point(&self, agent: AgentId, agent_loop: &mut AgentLoop, decision: &Decision) {
        self.live.note_model(agent, &decision.model.id);
        if agent_loop.config().model == decision.model.id {
            return;
        }
        let mut config = agent_loop.config().clone();
        self.limits.configure(&mut config, &decision.model.id);
        *agent_loop = AgentLoop::new(config);
    }

    /// Have Gate decide whether the turn on `agent_loop` with `tools`,
    /// asking for `category` of a difficulty, is led
    /// ([`GateLive::way`]), and make it a lead's if so (see
    /// [`LeadProfile::apply`]). Returns whether it is led (`None` when
    /// the agent never leads) and whether its calls must stay within the
    /// lead's limits.
    fn lead(
        &self,
        (category, difficulty): (PromptCategory, Option<Difficulty>),
        agent_loop: &mut AgentLoop,
        tools: &mut Vec<ToolDefinition>,
    ) -> (Option<bool>, bool) {
        let Some(lead) = self.lead else {
            return (None, false);
        };
        let mut config = agent_loop.config().clone();
        let way = self.live.way((category, difficulty), &config.model, true);
        if way.explored {
            info!(led = way.led, %category, "Gate explores the other way of working");
        }
        let before = config.system_prompt.len();
        let enforced = lead.apply(way.led, &mut config, tools);
        if config.system_prompt.len() != before {
            *agent_loop = AgentLoop::new(config);
        }
        (Some(way.led), enforced)
    }

    /// Show the model and route of `decision` in the status line.
    async fn show(&self, ui: &AgentUi, decision: &Decision) {
        let mut models = self.models.clone();
        if let UiCommand::SetModels { route, .. } = &mut models {
            *route = Some(decision.model.route_label());
        }
        ui.send_app(models).await;
    }
}

/// The chat line saying where Gate sent a turn asking for `category`,
/// e.g. `gate → claude-sonnet-4-6 (claude plan) · question, default`.
fn route_line(decision: &Decision, category: PromptCategory) -> String {
    format!(
        "gate → {} · {category}, {}",
        decision.model.route_label(),
        decision.reason
    )
}

/// How Gate records a turn's prompt: who classifies it, and why the turn
/// runs on its model. A turn on a pinned model runs beside the
/// classifier; a turn Gate routes waits for its category.
pub(super) struct Routing {
    /// Model that classifies the prompt
    pub classifier: String,
    /// Why the turn runs on its model, until Gate decides
    pub reason: RouteReason,
}

/// An agent's work that ended, handing its session back.
pub(super) struct WorkDone {
    pub agent_id: AgentId,
    /// `None` when the task panicked; the session is reopened from the
    /// record the next time it is needed.
    pub session: Option<AgentSession>,
    /// Why a turn failed, for the event loop to report.
    pub failure: Option<String>,
    /// Messages sent into the turn that the model never got, to run next.
    pub unseen: Vec<(SteerSource, String)>,
    /// Models Gate picked whose route refused the user's sign-in or plan.
    pub refused_models: Vec<String>,
}

/// Run `task` on `session` in the background, reporting to `ui`, and send
/// the session back on `done`.
pub(super) fn spawn(
    session: AgentSession,
    task: Task,
    ui: AgentUi,
    cancel: CancellationToken,
    done: mpsc::Sender<WorkDone>,
) {
    let agent_id = session.agent_id();
    tokio::spawn(async move {
        let work = tokio::spawn(run(session, task, ui, cancel));
        let (session, outcome) = match work.await {
            Ok((session, outcome)) => (Some(session), outcome),
            Err(e) => {
                error!(%agent_id, error = %e, "Agent work panicked");
                (
                    None,
                    Outcome {
                        failure: Some(format!("Agent stopped unexpectedly: {e}")),
                        ..Outcome::default()
                    },
                )
            }
        };
        let _ = done
            .send(WorkDone {
                agent_id,
                session,
                failure: outcome.failure,
                unseen: outcome.unseen,
                refused_models: outcome.refused,
            })
            .await;
    });
}

/// How a piece of work ended.
#[derive(Default)]
struct Outcome {
    failure: Option<String>,
    unseen: Vec<(SteerSource, String)>,
    refused: Vec<String>,
}

async fn run(
    mut session: AgentSession,
    task: Task,
    ui: AgentUi,
    cancel: CancellationToken,
) -> (AgentSession, Outcome) {
    let outcome = match task {
        Task::Turn {
            prompt,
            agent_loop,
            tools,
            steering,
            limits,
            recorder,
            gate,
            routing,
        } => {
            let turn = TurnParts {
                agent_loop,
                tools,
                steering: &steering,
                limits,
                recorder,
                gate,
                routing,
            };
            run_prompt(&mut session, prompt, turn, &ui, &cancel).await
        }
        Task::Command { command, cols } => {
            run_user_command(&mut session, &command, cols, &ui).await;
            Outcome::default()
        }
        Task::Compact { agent_loop, focus } => {
            run_compact(&mut session, &agent_loop, focus.as_deref(), &ui, &cancel).await;
            Outcome::default()
        }
    };
    (session, outcome)
}

/// What a turn runs on, besides the session.
struct TurnParts<'a> {
    agent_loop: Box<AgentLoop>,
    tools: Vec<ToolDefinition>,
    steering: &'a Steering,
    limits: LongRunSettings,
    recorder: Option<Arc<RunRecorder>>,
    gate: Option<Box<GateTurn>>,
    routing: Box<Routing>,
}

/// Record `text` on `session` as a prompt from the user, not classified:
/// a message sent into a running turn, which is part of that turn. The
/// kernels' record observer shows the entry in the Record panel.
pub(super) async fn record_user_prompt(session: &AgentSession, text: String) -> Result<(), String> {
    record_prompt(session, &PromptPayload::new(text))
        .await
        .map(|_| ())
}

/// Record `prompt` on `session` as a prompt from the user, returning the
/// transaction and the record entry it became.
pub(crate) async fn record_prompt(
    session: &AgentSession,
    prompt: &PromptPayload,
) -> Result<(Transaction, RecordEntry), String> {
    let prompt_tx = Transaction::user_prompt_with(session.agent_id(), prompt);
    let (tx, token) = enqueue_and_dequeue(session, prompt_tx)?;
    let result = session
        .kernel
        .process_dequeued(tx.clone(), token)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to persist prompt record via kernel");
            format!("Kernel error: {e}")
        })?;
    Ok((tx, result.entry))
}

/// Record `prompt` as typed and run a turn answering it. The model gets
/// the files it mentions as well. A turn on a model the user pinned is
/// recorded with its route only: Gate neither classifies it nor records
/// how it went. Otherwise the prompt is recorded with its category and
/// route, and the turn ends with an `Outcome` saying how it went; a
/// prompt the classifier could not label fails the turn. When Gate
/// routes the turn, it picks the model once the prompt is classified;
/// when that model's route refuses the user's sign-in, the
/// turn carries on from where it stopped on Gate's next choice, and when
/// the turn fails through the model's own doing it carries on on the
/// next stronger model (see [`run_attempts`]).
async fn run_prompt(
    session: &mut AgentSession,
    prompt: Prompt,
    mut turn: TurnParts<'_>,
    ui: &AgentUi,
    cancel: &CancellationToken,
) -> Outcome {
    drain_stale_inbox(session).await;
    session.tasks.begin_request(
        session.agent_id(),
        session.kernel.agent_workspace(),
        &session.identity.name,
        &prompt.text,
        session
            .active_plan
            .clone()
            .filter(|_| session.building_plan),
    );
    let started = Instant::now();
    let model = turn.agent_loop.config().model.clone();
    gate_record::note_retry(session, &prompt.text, &model).await;
    let mut label = Label {
        text: prompt.text.clone(),
        classifier: turn.routing.classifier.clone(),
        auth_token: turn.agent_loop.config().auth_token.clone(),
        route: Route::turn(model, turn.routing.reason),
        led: None,
        difficulty: None,
    };
    let started_ms = super::processes::now_ms();
    let hub = turn.gate.as_ref().map(|gate| Arc::clone(&gate.hub));
    let mut turn_start = session.messages.len();
    let mut started = started;

    let (process_result, forwarded, attempts, recorded) =
        if turn.routing.reason == RouteReason::Pinned {
            begin_files(session, &prompt.text);
            if let Err(problem) = label.record(session, None).await {
                ui.send(UiCommand::ShowError(problem)).await;
            }
            session.messages.push(model_message(session, prompt).await);
            let (result, forwarded) = run_agent_turn(
                session,
                &turn.agent_loop,
                turn.tools.clone(),
                ui,
                cancel,
                turn.steering,
                turn.limits,
                turn.recorder.clone(),
            )
            .await;
            (result, forwarded, Attempts::default(), None)
        } else {
            let category = match label.classify(session).await {
                Ok(category) => category,
                Err(problem) => {
                    ui.send(UiCommand::ShowError(problem)).await;
                    ui.send(UiCommand::Complete).await;
                    return Outcome::default();
                }
            };
            if let Some(gate) = turn.gate.as_deref() {
                let agent = session.agent_id();
                if route_turn(gate, agent, &mut turn.agent_loop, &mut label, category, ui).await {
                    let (led, enforced) = gate.lead(
                        (category, label.difficulty),
                        &mut turn.agent_loop,
                        &mut turn.tools,
                    );
                    label.led = led;
                    session.lead = enforced;
                }
            }
            let recorded = match label.record(session, Some(category)).await {
                Ok(recorded) => (recorded, category),
                Err(problem) => {
                    ui.send(UiCommand::ShowError(problem)).await;
                    ui.send(UiCommand::Complete).await;
                    return Outcome::default();
                }
            };
            begin_files(session, &prompt.text);
            if let Some(files) = &session.file_checkpoints {
                files.note_prompt_tx(session.agent_id(), recorded.0);
            }
            session.messages.push(model_message(session, prompt).await);
            let asked = Asked {
                task: &label.text,
                prompt_tx: recorded.0,
                category,
                difficulty: label.difficulty,
                led: label.led,
                turn_start,
                started,
                started_ms,
            };
            let (result, forwarded, attempts) =
                run_attempts(session, &mut turn, asked, ui, cancel).await;
            // The outcome below is the last attempt's.
            let mut recorded = Some(recorded);
            if let Some((prompt_tx, start, at)) = attempts.moved {
                recorded = Some((prompt_tx, category));
                turn_start = start;
                started = at;
            }
            if attempts.settled {
                recorded = None;
            }
            (result, forwarded, attempts, recorded)
        };
    if let Some(files) = &session.file_checkpoints {
        files.end_turn(session.agent_id());
    }
    session.usage.add(&forwarded.usage);
    if let Some(path) = &forwarded.plan_path {
        session.active_plan = Some(path.clone());
        session.building_plan = false;
    }
    let (seen, unseen) = turn.steering.finish(
        process_result
            .as_ref()
            .map_or(&[][..], |result| result.messages.as_slice()),
    );
    let cancelled = cancel.is_cancelled();
    if let Some(rec) = &turn.recorder {
        super::trace::end_turn(rec, &process_result, cancelled);
    }
    if cancelled {
        ui.send(UiCommand::ShowWarning("Turn cancelled".to_string()))
            .await;
    }
    let signal = if cancelled || forwarded.credentials_rejected {
        OutcomeSignal::Cancelled
    } else {
        attempts
            .signal
            .or_else(|| ended_signal(&process_result, turn_start))
            .unwrap_or(OutcomeSignal::Unavailable)
    };

    let summary = process_result
        .as_ref()
        .ok()
        .and_then(|result| summary_of(&result.total_text));
    let failed = process_result.is_err() && !cancelled;
    // What the last attempt left, for Gate's attempt log.
    let left = process_result
        .as_ref()
        .ok()
        .filter(|_| hub.is_some())
        .map(|result| gate_log::evidence_of(result, turn_start, attempts.changed.as_deref()));
    session
        .tasks
        .end_request(
            session.agent_id(),
            |items| request_state(cancelled, failed, items),
            summary,
        )
        .await;
    let failure = match process_result {
        Ok(result) => {
            handle_agent_success(session, result, &forwarded, ui, &seen).await;
            None
        }
        Err(e) if cancelled => {
            info!(error = %e, "Cancelled turn ended with an error");
            ui.send(UiCommand::Complete).await;
            None
        }
        Err(e) => {
            error!(error = %e, "Agent loop failed");
            Some(e.to_string())
        }
    };
    if let Some((prompt_tx, category)) = recorded {
        let outcome = OutcomePayload {
            prompt_tx,
            category,
            model: turn.agent_loop.config().model.clone(),
            success: signal.success(),
            signal,
            cost_usd: attempts.cost_usd + forwarded.usage.cost_usd,
            subtask_cost_usd: hub.as_ref().map_or(0.0, |hub| {
                hub.subtask_cost_usd(session.agent_id(), started_ms)
            }),
            latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
        if let Some(hub) = &hub {
            let terms = Terms {
                difficulty: label.difficulty,
                led: label.led,
                routed: true,
            };
            hub.gate
                .log_attempt(session.agent_id(), &outcome, terms, left);
        }
        gate_record::record(&session.kernel, session.agent_id(), &outcome).await;
    }
    Outcome {
        failure,
        unseen,
        refused: attempts.refused,
    }
}

/// Have Gate pick the model of `agent`'s turn asking for `category`,
/// point `agent_loop` at it, show the choice, and make it the route
/// `label` records. Returns whether Gate had a model to pick.
async fn route_turn(
    gate: &GateTurn,
    agent: AgentId,
    agent_loop: &mut AgentLoop,
    label: &mut Label,
    category: PromptCategory,
    ui: &AgentUi,
) -> bool {
    let Some(decision) = gate.decide(category, label.difficulty, &[]) else {
        return false;
    };
    info!(
        model = %decision.model.id,
        %category,
        reason = %decision.reason,
        "Gate routed the turn"
    );
    gate.point(agent, agent_loop, &decision);
    ui.send(system_message(route_line(&decision, category)))
        .await;
    gate.show(ui, &decision).await;
    label.route = decision.route(RouteLevel::Turn);
    true
}

/// What a turn's prompt is recorded with besides its category.
struct Label {
    text: String,
    classifier: String,
    auth_token: Option<String>,
    route: Route,
    /// Whether Gate had the turn led; `None` when it did not decide
    led: Option<bool>,
    /// How hard the classifier judged the work, once it said
    difficulty: Option<Difficulty>,
}

impl Label {
    /// The prompt's category, as the classifier says; its difficulty is
    /// kept on `self`.
    ///
    /// # Errors
    /// Says why when the classifier gave no category and difficulty.
    async fn classify(&mut self, session: &AgentSession) -> Result<PromptCategory, String> {
        let label = gate_record::classify_prompt(
            &session.kernel,
            &self.classifier,
            &self.text,
            self.auth_token.clone(),
        )
        .await?;
        self.difficulty = Some(label.difficulty);
        Ok(label.category)
    }

    /// Record the prompt on `session` as asking for `category`, `None`
    /// when nothing classified it, returning its transaction hash.
    async fn record(
        &self,
        session: &AgentSession,
        category: Option<PromptCategory>,
    ) -> Result<Hash, String> {
        let payload = PromptPayload {
            text: self.text.clone(),
            category,
            classifier: category
                .map(|_| self.classifier.clone())
                .unwrap_or_default(),
            route: self.route.clone(),
            kind: None,
            led: self.led,
            difficulty: self.difficulty,
        };
        let (tx, _) = record_prompt(session, &payload).await?;
        if let Some(files) = &session.file_checkpoints {
            files.note_prompt_tx(session.agent_id(), tx.hash);
        }
        Ok(tx.hash)
    }
}

/// Start checkpointing the files the turn answering `text` changes.
fn begin_files(session: &AgentSession, text: &str) {
    if let Some(files) = &session.file_checkpoints {
        files.begin_turn(
            session.agent_id(),
            session.started_ms,
            session.messages.len(),
            text,
        );
    }
}

/// The signal of a turn that ran to its end with `result`, whose messages
/// from `turn_start` on are the turn's own; `None` when nothing says how
/// it went, and the judge has to. A turn the model's provider cut short
/// is [`OutcomeSignal::Unavailable`] whatever it did before, since its
/// work stopped where the model did not choose to stop it.
pub(crate) fn ended_signal(
    result: &Result<aura_agent::AgentLoopResult, aura_agent::AgentError>,
    turn_start: usize,
) -> Option<OutcomeSignal> {
    match result {
        Err(e) if e.is_unavailable() => Some(OutcomeSignal::Unavailable),
        Err(_) => Some(OutcomeSignal::Error),
        Ok(result) if result.llm_error.is_some() && result.llm_unavailable => {
            Some(OutcomeSignal::Unavailable)
        }
        Ok(result) => {
            let errored = result.llm_error.is_some() && result.total_text.trim().is_empty();
            let messages = result.messages.get(turn_start..).unwrap_or_default();
            turn_signal(errored, last_check(messages))
        }
    }
}

/// How a turn that ran to its end with `result` failed, for the judge:
/// its error, or its last check and what the check printed. `None` when
/// it did neither.
pub(crate) fn failure_text(
    result: &Result<aura_agent::AgentLoopResult, aura_agent::AgentError>,
    turn_start: usize,
) -> Option<String> {
    let result = match result {
        Err(e) => return Some(format!("The run ended in an error: {e}")),
        Ok(result) => result,
    };
    if let Some(error) = result
        .llm_error
        .as_ref()
        .filter(|_| result.total_text.trim().is_empty())
    {
        return Some(format!("The model call failed: {error}"));
    }
    let messages = result.messages.get(turn_start..).unwrap_or_default();
    last_check_seen(messages)
        .filter(|check| !check.passed)
        .map(|check| {
            format!(
                "The last test or build command it ran, `{}`, failed. What the command printed:\n{}",
                check.command, check.output
            )
        })
}

/// The attempts of a turn before its last: on models that refused the
/// user's sign-in, and on weaker models that failed.
#[derive(Default)]
struct Attempts {
    /// Models that refused
    refused: Vec<String>,
    /// What the refused attempts since the last recorded outcome cost
    cost_usd: f64,
    /// Set once the turn went on to a stronger model: the prompt its last
    /// attempt answers, where that attempt's messages start in the
    /// session, and when it started
    moved: Option<(Hash, usize, Instant)>,
    /// Whether the last attempt's outcome is already recorded
    settled: bool,
    /// What the last attempt ended with once Gate's follow-up had its
    /// say: a harness check's result, or the judge's verdict on unchecked
    /// work. `None` when Gate did not follow it up
    signal: Option<OutcomeSignal>,
    /// The files the last attempt changed by any means, when the harness
    /// compared the workspace before and after
    changed: Option<Vec<String>>,
}

/// The prompt a turn answers, for the attempts at it.
struct Asked<'a> {
    /// What the user asked
    task: &'a str,
    /// Its recorded prompt
    prompt_tx: Hash,
    category: PromptCategory,
    /// How hard the classifier judged it
    difficulty: Option<Difficulty>,
    /// Whether Gate had the turn led
    led: Option<bool>,
    /// Where the turn's messages start in the session
    turn_start: usize,
    started: Instant,
    /// When it started, in Unix milliseconds
    started_ms: u64,
}

/// Run the turn of the prompt `asked`. When Gate routes it, Gate is asked
/// again while a route refuses the user's sign-in. An attempt that ran
/// no check of its own gets Gate's follow-up first (see
/// [`gate_followup`]): when it changed files it is asked once to check
/// them, then the harness runs the project's tests, and work still
/// unchecked gets the judge's verdict. Work that is hard or shows a sign of
/// failure is reviewed against the request before it stands; what the
/// review finds unfinished goes back to the same model once. An attempt
/// that fails (a model error, its last check or the harness's failing,
/// or the judge finding its work unfinished) is handled here:
///
/// 1. its `Outcome` is recorded and, for a failed check or an error,
///    Gate's judge says whether the model was at fault
///    ([`gate_record::judge_failed`]);
/// 2. a failure the judge excuses ends the turn where it is;
/// 3. otherwise the turn carries on in the same conversation on the
///    next stronger model, told what the judge found, under a prompt of
///    its own recorded with route reason `escalate`.
///
/// A failure with no judge and no stronger model is left for the caller
/// to record.
async fn run_attempts(
    session: &mut AgentSession,
    turn: &mut TurnParts<'_>,
    asked: Asked<'_>,
    ui: &AgentUi,
    cancel: &CancellationToken,
) -> (
    Result<aura_agent::AgentLoopResult, aura_agent::AgentError>,
    ForwarderState,
    Attempts,
) {
    let mut attempts = Attempts::default();
    let gate = turn.gate.take();
    let agent = session.agent_id();
    let category = asked.category;
    // Watched so the judge can see what a failed attempt changed.
    let files = session.file_checkpoints.clone().filter(|_| gate.is_some());
    if let Some(files) = &files {
        files.begin_attempt(agent);
    }
    let (mut prompt_tx, mut turn_start, mut started) =
        (asked.prompt_tx, asked.turn_start, asked.started);
    // The workspace as it is now, so a review sees everything the turn
    // changes, by its commands as well as its file tools.
    let before = match gate.as_ref() {
        Some(gate)
            if aura_agent_gate::router::verifiable(category)
                && may_review(
                    &gate.live,
                    &gate.candidates,
                    &turn.agent_loop.config().model,
                ) =>
        {
            Before::of_workspace(session.kernel.agent_workspace()).await
        }
        _ => None,
    };
    // The follow-up steps of the attempt on `follow.0`; a new model starts
    // them afresh.
    let mut follow = (String::new(), FollowUp::default());
    let (result, forwarded) = 'attempts: loop {
        let (result, forwarded) = run_agent_turn(
            session,
            &turn.agent_loop,
            turn.tools.clone(),
            ui,
            cancel,
            turn.steering,
            turn.limits,
            turn.recorder.clone(),
        )
        .await;
        let Some(gate) = gate.as_ref() else {
            break (result, forwarded);
        };
        if cancel.is_cancelled() {
            break (result, forwarded);
        }
        let model = turn.agent_loop.config().model.clone();
        if forwarded.credentials_rejected {
            attempts.refused.push(model.clone());
            let Some(next) = gate.decide(category, asked.difficulty, &attempts.refused) else {
                break (result, forwarded);
            };
            attempts.cost_usd += forwarded.usage.cost_usd;
            session.usage.add(&forwarded.usage);
            if let Ok(partial) = &result {
                session.messages.clone_from(&partial.messages);
            }
            info!(from = %model, to = %next.model.id, "Gate re-routed the turn");
            ui.send(system_message(format!(
                "{model} refused your sign-in; Gate skips it until your sign-ins change.\n{}",
                route_line(&next, category)
            )))
            .await;
            gate.point(agent, &mut turn.agent_loop, &next);
            gate.show(ui, &next).await;
            continue;
        }

        let mut signal = ended_signal(&result, turn_start);
        // What the harness's check printed when it failed, and what the
        // judge found wrong with unchecked work.
        let (mut harness, mut unchecked) = (None, None);
        if let Ok(partial) = &result {
            if follow.0 != model {
                let workspace = session.kernel.agent_workspace();
                let fresh = FollowUp::new(&gate.live, &gate.candidates, &model, &workspace)
                    .since(before.clone())
                    .after(&follow.1);
                follow = (model.clone(), fresh);
            }
            let edits = || {
                let workspace = session.kernel.agent_workspace();
                files
                    .as_ref()
                    .map(|files| files.attempt(agent).diff(Some(&workspace)))
                    .unwrap_or_default()
            };
            // What the same model is told when it carries on, and what
            // the user is told of that.
            let mut carry_on = None;
            loop {
                let at = At {
                    category,
                    difficulty: asked.difficulty,
                    signal,
                    turn_start,
                };
                match follow.1.next(partial, at).await {
                    Step::Stop => break,
                    Step::Nudge => {
                        carry_on = Some((
                            nudge_prompt(),
                            format!(
                                "gate: {} changed files without running a test or build; asking it to check its change",
                                aura_model::model_catalog::route_label(&model)
                            ),
                        ));
                        break;
                    }
                    Step::Repair(signs) => {
                        carry_on = Some((
                            sign_repair_message(&signs),
                            sign_repair_notice(&model, &signs),
                        ));
                        break;
                    }
                    Step::Review { why, act } => {
                        let ended = OutcomePayload {
                            prompt_tx,
                            category,
                            model: model.clone(),
                            success: None,
                            signal: signal.unwrap_or(OutcomeSignal::Unavailable),
                            cost_usd: attempts.cost_usd + forwarded.usage.cost_usd,
                            subtask_cost_usd: gate.hub.subtask_cost_usd(agent, asked.started_ms),
                            latency_ms: u64::try_from(started.elapsed().as_millis())
                                .unwrap_or(u64::MAX),
                        };
                        let edits = edits();
                        let shown = Reviewing {
                            task: asked.task,
                            result: partial,
                            turn_start,
                            category,
                            edits: &edits,
                            why: &why,
                            act,
                        };
                        let auth_token = turn.agent_loop.config().auth_token.clone();
                        let reviewed = follow
                            .1
                            .review(session, &gate.live, &ended, &shown, auth_token)
                            .await;
                        let Some(reviewed) = reviewed else {
                            continue;
                        };
                        ui.send(system_message(reviewed.describe())).await;
                        if reviewed.unfinished() {
                            if follow.1.send_back(partial) {
                                carry_on = Some((
                                    repair_message(&reviewed.findings),
                                    repair_notice(&model),
                                ));
                                break;
                            }
                            signal = Some(OutcomeSignal::JudgedFail);
                            unchecked = Some(reviewed.findings);
                        } else if signal.is_none() {
                            signal = reviewed.verdict();
                        }
                    }
                    Step::HarnessCheck => {
                        let Some(command) = follow.1.check().map(str::to_string) else {
                            continue;
                        };
                        ui.send(system_message(format!(
                            "gate: running `{command}` to check the change"
                        )))
                        .await;
                        let check = harness_check(session, &command).await;
                        ui.send(system_message(check.describe())).await;
                        if let Some(checked) = check_signal(&check) {
                            signal = Some(checked);
                            harness = (checked == OutcomeSignal::ChecksFailed)
                                .then(|| check_review(&check));
                        }
                    }
                    Step::JudgeUnverified(suspicions) => {
                        let Some(judge) = follow.1.judge().map(str::to_string) else {
                            continue;
                        };
                        let ended = OutcomePayload {
                            prompt_tx,
                            category,
                            model: model.clone(),
                            success: None,
                            signal: signal.unwrap_or(OutcomeSignal::Unavailable),
                            cost_usd: attempts.cost_usd + forwarded.usage.cost_usd,
                            subtask_cost_usd: gate.hub.subtask_cost_usd(agent, asked.started_ms),
                            latency_ms: u64::try_from(started.elapsed().as_millis())
                                .unwrap_or(u64::MAX),
                        };
                        let diff = follow.1.diff(partial, edits()).await;
                        let shown = Unchecked {
                            task: asked.task,
                            diff: &diff,
                            result: partial,
                            turn_start,
                            suspicions: &suspicions,
                        };
                        let auth_token = turn.agent_loop.config().auth_token.clone();
                        let judged =
                            judge_unchecked(session, &judge, &ended, &shown, auth_token).await;
                        if let Some((verdict, judged)) = judged {
                            ui.send(system_message(judged.describe())).await;
                            if verdict == OutcomeSignal::JudgedFail || signal.is_none() {
                                signal = Some(verdict);
                            }
                            if verdict == OutcomeSignal::JudgedFail {
                                unchecked = Some(judged.verdict.reasons);
                            }
                        }
                    }
                }
            }
            if let Some((message, notice)) = carry_on {
                // The same model carries on in the same conversation.
                session.messages.clone_from(&partial.messages);
                session.todos.clone_from(&partial.todos);
                record_response(session, &partial.total_text).await;
                if let Err(problem) = record_user_prompt(session, message.clone()).await {
                    warn!(%problem, "Could not record Gate's message to the model");
                }
                session.usage.add(&forwarded.usage);
                attempts.cost_usd += forwarded.usage.cost_usd;
                session.messages.push(Message::user(message));
                info!(%model, %notice, "Gate has the turn carry on");
                ui.send(system_message(notice)).await;
                let mut config = turn.agent_loop.config().clone();
                config.initial_todos.clone_from(&session.todos);
                *turn.agent_loop = AgentLoop::new(config);
                continue 'attempts;
            }
        }
        let signal = signal.unwrap_or(OutcomeSignal::Unavailable);
        attempts.signal = Some(signal);
        if !calls_for_stronger(signal) {
            break (result, forwarded);
        }
        let failed = OutcomePayload {
            prompt_tx,
            category,
            model: model.clone(),
            success: signal.success(),
            signal,
            cost_usd: attempts.cost_usd + forwarded.usage.cost_usd,
            subtask_cost_usd: gate.hub.subtask_cost_usd(agent, asked.started_ms),
            latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
        let stronger = |live: &GateLive| {
            // Only a turn whose conversation survived can be carried on.
            result
                .is_ok()
                .then(|| live.escalate(RouteLevel::Turn, category, &model, &gate.candidates))
                .flatten()
        };
        let judge = gate_record::judge_for(&gate.live, &gate.candidates, &failed);
        let mut next = None;
        if judge.is_none() {
            next = stronger(&gate.live);
            if next.is_none() {
                break (result, forwarded);
            }
        }
        let terms = Terms {
            difficulty: asked.difficulty,
            led: asked.led,
            routed: true,
        };
        let left = result
            .as_ref()
            .ok()
            .map(|result| follow.1.evidence(result, turn_start));
        gate.live.log_attempt(agent, &failed, terms, left);
        gate_record::record(&session.kernel, agent, &failed).await;
        attempts.settled = true;
        let mut review = unchecked;
        if let Some(judge) = judge {
            let workspace = session.kernel.agent_workspace();
            let edits = files
                .as_ref()
                .map(|files| files.attempt(agent).diff(Some(&workspace)))
                .unwrap_or_default();
            let diff = match &result {
                Ok(partial) => follow.1.diff(partial, edits).await,
                Err(_) => edits,
            };
            let failure = harness
                .clone()
                .or_else(|| failure_text(&result, turn_start))
                .unwrap_or_default();
            let shown = Failed {
                task: asked.task,
                diff: &diff,
                reply: result.as_ref().map_or("", |r| r.total_text.as_str()),
                failure: &failure,
            };
            let auth_token = turn.agent_loop.config().auth_token.clone();
            let judged =
                gate_record::judge_failed(&session.kernel, &judge, &failed, &shown, auth_token)
                    .await;
            if let Some((_, judged)) = judged {
                ui.send(system_message(judged.describe())).await;
                if !judged.verdict.at_fault {
                    break (result, forwarded);
                }
                review = Some(judged.verdict.reasons);
            }
            next = stronger(&gate.live);
        }
        let review = joined(harness, review);
        let (Some(next), Ok(partial)) = (next, &result) else {
            break (result, forwarded);
        };

        // The stronger model carries on from what the attempt left.
        let takeover = format!(
            "{}{}",
            SteerSource::Harness.prefix(),
            takeover_prompt(
                &aura_model::model_catalog::route_label(&model),
                signal,
                review.as_deref()
            )
        );
        let payload = PromptPayload {
            category: Some(category),
            route: next.route(RouteLevel::Turn),
            ..PromptPayload::new(takeover.clone())
        };
        session.messages.clone_from(&partial.messages);
        session.todos.clone_from(&partial.todos);
        record_response(session, &partial.total_text).await;
        let recorded = match record_prompt(session, &payload).await {
            Ok((tx, _)) => tx.hash,
            Err(problem) => {
                warn!(%problem, "Could not record the stronger model's prompt");
                break (result, forwarded);
            }
        };
        session.usage.add(&forwarded.usage);
        attempts.cost_usd = 0.0;
        attempts.settled = false;
        attempts.signal = None;
        (prompt_tx, turn_start, started) = (recorded, session.messages.len(), Instant::now());
        attempts.moved = Some((prompt_tx, turn_start, started));
        session.messages.push(Message::user(takeover));
        info!(from = %model, to = %next.model.id, %signal, "Gate moves the turn to a stronger model");
        ui.send(system_message(route_line(&next, category))).await;
        let mut config = turn.agent_loop.config().clone();
        config.initial_todos.clone_from(&session.todos);
        *turn.agent_loop = AgentLoop::new(config);
        gate.point(agent, &mut turn.agent_loop, &next);
        gate.show(ui, &next).await;
    };
    if let Some(files) = &files {
        let _ = files.take_attempt(agent);
    }
    attempts.changed = result
        .as_ref()
        .ok()
        .and_then(|result| follow.1.changed_paths(result))
        .map(<[String]>::to_vec);
    (result, forwarded, attempts)
}

/// The user message the model gets for `prompt`: its images, then its
/// text with the mentioned files from the agent's workspace appended.
async fn model_message(session: &AgentSession, prompt: Prompt) -> Message {
    let Prompt {
        text,
        mentions,
        images,
    } = prompt;
    let text = if mentions && !mentioned_paths(&text).is_empty() {
        let expanded = expand_mentions_in(session.kernel.agent_workspace(), text).await;
        if !expanded.attached.is_empty() {
            info!(files = ?expanded.attached, "Attached mentioned files");
        }
        expanded.text
    } else {
        text
    };
    if images.is_empty() {
        return Message::user(text);
    }
    info!(count = images.len(), "Attached pasted images");
    user_message_with_images(text, images)
}

/// A user message holding `images` and then `text`. Anthropic advises
/// putting images before the text that refers to them.
fn user_message_with_images(text: String, images: Vec<ImageAttachment>) -> Message {
    let mut content: Vec<ContentBlock> = images
        .into_iter()
        .map(|image| ContentBlock::Image {
            source: ImageSource {
                source_type: "base64".to_string(),
                media_type: image.media_type,
                data: image.data,
            },
        })
        .collect();
    content.push(ContentBlock::Text { text });
    Message::new(Role::User, content)
}

/// Replace the older history of `session` with a summary. Only the
/// conversation the model sees changes; the record keeps every message.
async fn run_compact(
    session: &mut AgentSession,
    agent_loop: &AgentLoop,
    focus: Option<&str>,
    ui: &AgentUi,
    cancel: &CancellationToken,
) {
    ui.send(UiCommand::SetStatus("Summarizing history...".to_string()))
        .await;
    let compacted = agent_loop
        .compact_history(
            &session.model_gateway,
            &session.messages,
            &session.todos,
            focus,
            Some(cancel),
        )
        .await;
    match compacted {
        Ok(compacted) => {
            info!(
                before = compacted.before_tokens,
                after = compacted.after_tokens,
                "Compacted history on request"
            );
            session.messages = compacted.messages;
            session.save_checkpoint().await;
            ui.send(UiCommand::Compacted(format!(
                "Summarized older history: ~{} → ~{} tokens",
                compacted.before_tokens, compacted.after_tokens
            )))
            .await;
        }
        Err(CompactHistoryError::TooShort) => {
            ui.send(system_message(
                "Nothing to summarize yet: the history is still short".to_string(),
            ))
            .await;
        }
        Err(CompactHistoryError::Cancelled) => {
            ui.send(UiCommand::ShowWarning("Summary cancelled".to_string()))
                .await;
        }
        Err(e) => {
            warn!(error = %e, "Compacting history failed");
            ui.send(UiCommand::ShowWarning(format!(
                "Could not summarize the history: {e}"
            )))
            .await;
        }
    }
    ui.send(UiCommand::Complete).await;
}

/// Clear transactions left in the agent's inbox by an earlier run.
pub(crate) async fn drain_stale_inbox(session: &AgentSession) {
    let store = session.kernel.store();
    let mut stale_count = 0;
    while let Ok(Some((token, tx))) = store.dequeue_tx(session.agent_id()) {
        warn!(
            stale_inbox_seq = token.inbox_seq(),
            stale_tx_type = ?tx.tx_type,
            "Discarding stale inbox transaction"
        );
        match session.kernel.process_dequeued(tx, token).await {
            Ok(_result) => {
                stale_count += 1;
            }
            Err(e) => {
                error!(error = %e, "Failed to clear stale transaction via kernel");
                break;
            }
        }
        if stale_count > 10 {
            error!("Too many stale transactions, aborting drain");
            break;
        }
    }
}

/// Put `tx` through the agent's inbox, returning it with its dequeue token.
fn enqueue_and_dequeue(
    session: &AgentSession,
    tx: Transaction,
) -> Result<(Transaction, aura_store_record::DequeueToken), String> {
    let store = session.kernel.store();
    if let Err(e) = store.enqueue_tx(&tx) {
        error!(error = %e, "Failed to enqueue transaction");
        return Err(format!("Storage error: {e}"));
    }

    let (token, dequeued_tx) = match store.dequeue_tx(session.agent_id()) {
        Ok(Some(item)) => item,
        Ok(None) => {
            error!("Transaction was enqueued but not found in inbox");
            return Err("Storage error: the message was not queued".to_string());
        }
        Err(e) => {
            error!(error = %e, "Failed to dequeue transaction");
            return Err(format!("Storage error: {e}"));
        }
    };

    if dequeued_tx.hash != tx.hash {
        error!("Transaction mismatch after draining stale entries");
    }

    Ok((tx, token))
}
