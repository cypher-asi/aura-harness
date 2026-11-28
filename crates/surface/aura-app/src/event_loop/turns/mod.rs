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
use super::gate;
use super::gate::turn::{Ending, GateTurn, Routing};
use super::handlers::{handle_agent_success, run_agent_turn, system_message, TurnControls};
use super::run_command::run_user_command;
use super::steering::{SteerSource, Steering};
use crate::long_run::LongRunSettings;
use aura_agent::{AgentError, AgentLoop, AgentLoopResult, CompactHistoryError};
use aura_agent_trace::RunRecorder;
use aura_core_types::{AgentId, Hash, PromptPayload, RecordEntry, Route, RouteReason, Transaction};
use aura_gate::live;
use aura_model::{ContentBlock, ImageSource, Message, Role, ToolDefinition};
use aura_ui::events::ImageAttachment;
use aura_ui::UiCommand;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

mod board;
mod compact;
mod messages;
mod record;

use board::{begin_task, end_request};
use compact::run_compact;
pub(super) use messages::{begin_files, model_message};
pub(crate) use record::{drain_stale_inbox, record_prompt};
pub(super) use record::{record_turn_prompt, record_user_prompt};

/// A message for the model, with what the user attached to it.
pub(super) struct Prompt {
    /// The message as typed; the chat and the record keep this
    pub text: String,
    /// Whether `@path` words in `text` attach files. Only text the user
    /// typed has mentions; a harness notice may quote `@` words verbatim.
    pub mentions: bool,
    /// Images pasted into the message
    pub images: Vec<ImageAttachment>,
    /// What its turn is on the agent's task board
    pub request: Request,
    /// Context the model gets ahead of `text`, which the chat and the
    /// record leave out (a coordinator's channel digest)
    pub preamble: Option<String>,
}

/// What a prompt's turn is on the agent's task board (see
/// [`super::tasks`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Request {
    /// A new request
    New,
    /// The request that builds the agent's plan (`/build` or "implement")
    Build,
    /// No request: a harness notice carries on with the agent's latest
    /// task, which it is about
    Resume,
}

impl Prompt {
    /// A message typed by the user, with the images pasted into it.
    pub(super) const fn typed(text: String, images: Vec<ImageAttachment>) -> Self {
        Self {
            text,
            mentions: true,
            images,
            request: Request::New,
            preamble: None,
        }
    }

    /// A message written by the harness for something the user asked,
    /// given to the model as is.
    pub(super) const fn plain(text: String) -> Self {
        Self {
            text,
            mentions: false,
            images: Vec::new(),
            request: Request::New,
            preamble: None,
        }
    }

    /// The message that starts building the agent's plan.
    pub(super) const fn build(text: String) -> Self {
        Self {
            text,
            mentions: false,
            images: Vec::new(),
            request: Request::Build,
            preamble: None,
        }
    }

    /// A notice from the harness, such as a background job ending.
    pub(super) const fn notice(text: String) -> Self {
        Self {
            text,
            mentions: false,
            images: Vec::new(),
            request: Request::Resume,
            preamble: None,
        }
    }

    /// A message from another agent's inbox (see [`super::inbox`]): a
    /// [`Request::New`] for work handed out, else a [`Request::Resume`].
    pub(super) const fn inbox(text: String, request: Request) -> Self {
        Self {
            text,
            mentions: false,
            images: Vec::new(),
            request,
            preamble: None,
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
        /// Whether the agent is in plan mode, where a request revises its
        /// open plan (see [`begin_task`])
        planning: bool,
        /// Whether the software factory builds the plan this turn writes
        /// on its own, so nobody is asked to say "implement"
        auto_build: bool,
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
            planning,
            auto_build,
            agent_loop,
            tools,
            steering,
            limits,
            recorder,
            gate,
            routing,
        } => {
            let turn = TurnParts {
                auto_build,
                agent_loop,
                tools,
                steering: &steering,
                limits,
                recorder,
                gate,
                routing,
                ui: &ui,
                cancel: &cancel,
            };
            run_prompt(&mut session, prompt, planning, turn).await
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
pub(super) struct TurnParts<'a> {
    /// Whether the software factory builds the plan the turn writes
    pub auto_build: bool,
    pub agent_loop: Box<AgentLoop>,
    pub tools: Vec<ToolDefinition>,
    pub steering: &'a Steering,
    pub limits: LongRunSettings,
    pub recorder: Option<Arc<RunRecorder>>,
    /// What Gate routes the turn among, when it routes it
    pub gate: Option<Box<GateTurn>>,
    /// How Gate records the turn's prompt
    pub routing: Box<Routing>,
    /// Where the turn reports
    pub ui: &'a AgentUi,
    /// Stops the turn
    pub cancel: &'a CancellationToken,
}

impl TurnParts<'_> {
    /// Run one attempt at the turn on `session`, with the turn's agent
    /// loop and tools as they stand.
    pub(super) async fn attempt(
        &self,
        session: &AgentSession,
    ) -> (Result<AgentLoopResult, AgentError>, ForwarderState) {
        run_agent_turn(
            session,
            &self.agent_loop,
            self.tools.clone(),
            TurnControls {
                ui: self.ui,
                cancel: self.cancel,
                steering: self.steering,
                limits: self.limits,
                recorder: self.recorder.clone(),
                auto_build: self.auto_build,
            },
        )
        .await
    }
}

/// How a turn's last attempt ended, and what Gate records of the turn
/// once that is handled.
pub(super) struct Ran {
    pub result: Result<AgentLoopResult, AgentError>,
    pub forwarded: ForwarderState,
    /// The outcome Gate records once the turn's result is handled; `None`
    /// on a pinned model, and when the last attempt's outcome is already
    /// recorded
    pub ending: Option<Ending>,
    /// Models Gate picked whose route refused the user's sign-in or plan
    pub refused: Vec<String>,
}

/// Record `prompt` as typed and run a turn answering it. The model gets
/// the files it mentions as well. A turn on a model the user pinned is
/// recorded with its route only: Gate neither classifies it nor records
/// how it went. Otherwise Gate labels the turn (see
/// [`gate::turn`](super::gate::turn)): the prompt is recorded with its
/// category and route, and the turn ends with an `Outcome` saying how it
/// went; a prompt the classifier could not label fails the turn.
/// `planning` is whether the agent is in plan mode.
async fn run_prompt(
    session: &mut AgentSession,
    prompt: Prompt,
    planning: bool,
    mut turn: TurnParts<'_>,
) -> Outcome {
    drain_stale_inbox(session).await;
    begin_task(session, &prompt, planning).await;
    let started = Instant::now();
    let model = turn.agent_loop.config().model.clone();
    live::note_retry(&session.kernel, session.agent_id(), &prompt.text, &model).await;
    let ran = if turn.routing.reason == RouteReason::Pinned {
        run_pinned(session, prompt, &turn, model).await
    } else {
        match gate::turn::run(session, prompt, &mut turn, started).await {
            Some(ran) => ran,
            None => return Outcome::default(),
        }
    };
    finish(session, ran, &turn).await
}

/// Run the turn of `prompt` on `model`, which the user pinned: the prompt
/// is recorded with its route only.
async fn run_pinned(
    session: &mut AgentSession,
    prompt: Prompt,
    turn: &TurnParts<'_>,
    model: String,
) -> Ran {
    begin_files(session, &prompt.text);
    let recorded = PromptPayload {
        route: Route::turn(model, RouteReason::Pinned),
        ..PromptPayload::new(prompt.text.clone())
    };
    if let Err(problem) = record_turn_prompt(session, &recorded).await {
        turn.ui.send(UiCommand::ShowError(problem)).await;
    }
    session.messages.push(model_message(session, prompt).await);
    let (result, forwarded) = turn.attempt(session).await;
    Ran {
        result,
        forwarded,
        ending: None,
        refused: Vec::new(),
    }
}

/// Close the turn that ended as `ran`: keep what it left in `session`,
/// report how it ended, and record Gate's outcome of it once the result
/// is handled.
async fn finish(session: &mut AgentSession, ran: Ran, turn: &TurnParts<'_>) -> Outcome {
    let Ran {
        result,
        forwarded,
        ending,
        refused,
    } = ran;
    keep(session, &forwarded);
    let messages = result
        .as_ref()
        .map_or(&[][..], |result| result.messages.as_slice());
    let (seen, unseen) = turn.steering.finish(messages);
    let cancelled = turn.cancel.is_cancelled();
    if let Some(rec) = &turn.recorder {
        super::trace::end_turn(rec, &result, cancelled);
    }
    if cancelled {
        turn.ui
            .send(UiCommand::ShowWarning("Turn cancelled".to_string()))
            .await;
    }
    let stopped = cancelled || forwarded.credentials_rejected;
    let settled = ending.map(|ending| ending.settle(&result, stopped));
    end_request(session, &result, cancelled).await;
    let failure = match result {
        Ok(result) => {
            handle_agent_success(session, result, &forwarded, turn.ui, &seen).await;
            None
        }
        Err(e) if cancelled => {
            info!(error = %e, "Cancelled turn ended with an error");
            turn.ui.send(UiCommand::Complete).await;
            None
        }
        Err(e) => {
            error!(error = %e, "Agent loop failed");
            Some(e.to_string())
        }
    };
    if let Some(settled) = settled {
        settled.record(session, turn, &forwarded).await;
    }
    Outcome {
        failure,
        unseen,
        refused,
    }
}

/// Keep what the turn's last run, which `forwarded` followed, left in
/// `session` besides its conversation: what it used and the plan it
/// wrote. The files changed from here on are no longer the turn's.
fn keep(session: &mut AgentSession, forwarded: &ForwarderState) {
    if let Some(files) = &session.file_checkpoints {
        files.end_turn(session.agent_id());
    }
    session.usage.add(&forwarded.usage);
    if let Some(path) = &forwarded.plan_path {
        session.active_plan = Some(path.clone());
        session.building_plan = false;
    }
}
