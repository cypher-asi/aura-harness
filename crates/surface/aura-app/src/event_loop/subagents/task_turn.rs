//! A subtask's turn: running it to the end within its time limit, and
//! how it ended.

use super::*;
use crate::event_loop::agent_events::ForwarderState;

/// How a subtask's turn ended.
pub(super) struct TaskEnd {
    /// The final reply, or why there is none
    pub(super) reply: Result<String, String>,
    /// The recorded prompt and its category, when Gate routed the
    /// subtask and recorded it
    pub(super) prompt: Option<(Hash, PromptCategory)>,
    /// What the turn's end says about how it went; `None` when nothing
    /// does, and the judge has to
    pub(super) signal: Option<OutcomeSignal>,
    /// List-price cost of the turn's model calls
    pub(super) cost_usd: f64,
    /// Files the turn's file tools changed
    pub(super) files: Vec<String>,
    /// The commands the turn ran and its last test or build, as the
    /// judge of unchecked work is shown them
    pub(super) shown: (String, String),
}

impl TaskEnd {
    pub(super) const fn failed(reason: String) -> Self {
        Self {
            reply: Err(reason),
            prompt: None,
            signal: Some(OutcomeSignal::Error),
            cost_usd: 0.0,
            files: Vec::new(),
            shown: (String::new(), String::new()),
        }
    }
}

/// One turn of a subtask: `prompt`, recorded as `label` says, run on
/// `agent_loop` with `tools` and streamed into `ui`. A `label` Gate could
/// not give ends the turn before it starts.
pub(super) struct TaskTurn<'a> {
    pub(super) agent_loop: &'a AgentLoop,
    pub(super) tools: Vec<ToolDefinition>,
    pub(super) prompt: &'a str,
    pub(super) label: Result<SubtaskLabel, String>,
    pub(super) ui: &'a AgentUi,
    pub(super) cancel: &'a CancellationToken,
    pub(super) limits: LongRunSettings,
    pub(super) recorder: Option<Arc<RunRecorder>>,
}

/// Record `turn`'s prompt on `session` as its label says and run one
/// turn of it, streaming it into its UI.
pub(super) async fn run_task_turn(session: &mut AgentSession, turn: TaskTurn<'_>) -> TaskEnd {
    let turn_start = session.messages.len();
    let recorded = match turn.open(session).await {
        Ok(recorded) => recorded,
        Err(end) => return end,
    };
    let ending = TurnEnding {
        ui: turn.ui,
        cancel: turn.cancel,
        turn_start,
        limit: turn.limits.turn_timeout,
    };
    let (result, forwarded) = turn.run(session).await;
    ending.conclude(session, result, &forwarded, recorded).await
}

impl TaskTurn<'_> {
    /// Record the prompt on `session` as the label says, add it to the
    /// conversation and show the turn starting; the turn's end when there
    /// is no label or the prompt could not be recorded.
    async fn open(
        &self,
        session: &mut AgentSession,
    ) -> Result<Option<(Hash, PromptCategory)>, TaskEnd> {
        let label = match &self.label {
            Ok(label) => label,
            Err(problem) => {
                self.ui.send(UiCommand::ShowError(problem.clone())).await;
                self.ui.send(UiCommand::Complete).await;
                return Err(TaskEnd::failed(problem.clone()));
            }
        };
        let recorded = match label.record(session).await {
            Ok(recorded) => recorded,
            Err(problem) => return Err(TaskEnd::failed(format!("recording prompt: {problem}"))),
        };
        session.messages.push(Message::user(self.prompt));
        self.ui
            .send(UiCommand::TurnStarted {
                prompt: Some(self.prompt.to_string()),
            })
            .await;
        if let Some(line) = &label.shown {
            self.ui.send(system_message(line.clone())).await;
        }
        Ok(recorded)
    }

    /// Run the turn on `session`, told to wrap up shortly before its time
    /// is up, and close it on its trace.
    async fn run(
        self,
        session: &mut AgentSession,
    ) -> (
        Result<aura_agent::AgentLoopResult, aura_agent::AgentError>,
        ForwarderState,
    ) {
        if let Some(rec) = &self.recorder {
            rec.begin_turn(self.prompt);
        }
        // Only the harness sends to a spawn mid-task: that its time is
        // nearly up.
        let steering = Steering::new(self.cancel);
        let wrap_up = schedule_wrap_up(&self.limits, &steering);
        let (result, forwarded) = run_agent_turn(
            session,
            self.agent_loop,
            self.tools,
            TurnControls {
                ui: self.ui,
                cancel: self.cancel,
                steering: &steering,
                limits: self.limits,
                recorder: self.recorder.clone(),
            },
        )
        .await;
        if let Some(wrap_up) = wrap_up {
            wrap_up.abort();
        }
        steering.finish(&[]);
        if let Some(rec) = &self.recorder {
            crate::event_loop::trace::end_turn(rec, &result, self.cancel.is_cancelled());
        }
        (result, forwarded)
    }
}

/// Tell the turn through `steering` to wrap up shortly before its time,
/// when `limits` gives it enough for that to matter.
fn schedule_wrap_up(
    limits: &LongRunSettings,
    steering: &Steering,
) -> Option<tokio::task::JoinHandle<()>> {
    limits
        .turn_timeout
        .filter(|limit| *limit > WRAP_UP_BEFORE * 2)
        .map(|limit| {
            let steering = steering.clone();
            tokio::spawn(async move {
                tokio::time::sleep(limit - WRAP_UP_BEFORE).await;
                let notice = wrap_up_notice(WRAP_UP_BEFORE);
                steering
                    .send(SteerSource::Harness, notice.clone(), notice)
                    .await;
            })
        })
}

/// Why a turn that failed with `e` has no reply.
fn turn_failure(e: &aura_agent::AgentError, cancelled: bool, limit: Option<Duration>) -> String {
    match (e, limit) {
        _ if cancelled => "cancelled by the user".to_string(),
        (aura_agent::AgentError::Timeout(_), Some(limit)) => format!(
            "ran out of time: it stopped after {} minutes without a reply",
            limit.as_secs() / 60
        ),
        _ => e.to_string(),
    }
}

/// How a subtask turn begun at message `turn_start`, with time `limit`,
/// is closed.
struct TurnEnding<'a> {
    ui: &'a AgentUi,
    cancel: &'a CancellationToken,
    turn_start: usize,
    limit: Option<Duration>,
}

impl TurnEnding<'_> {
    /// How the turn that ran as `result` ended, its prompt `recorded`.
    async fn conclude(
        &self,
        session: &mut AgentSession,
        result: Result<aura_agent::AgentLoopResult, aura_agent::AgentError>,
        forwarded: &ForwarderState,
        recorded: Option<(Hash, PromptCategory)>,
    ) -> TaskEnd {
        let cancelled = self.cancel.is_cancelled();
        let signal = if cancelled || forwarded.credentials_rejected {
            Some(OutcomeSignal::Cancelled)
        } else {
            ended_signal(&result, self.turn_start)
        };
        let mut end = TaskEnd {
            reply: Ok(String::new()),
            prompt: recorded,
            signal,
            cost_usd: forwarded.usage.cost_usd,
            files: Vec::new(),
            shown: (String::new(), String::new()),
        };
        let result = match result {
            Ok(result) => result,
            Err(e) => {
                let reason = turn_failure(&e, cancelled, self.limit);
                self.ui.send(UiCommand::ShowWarning(reason.clone())).await;
                self.ui.send(UiCommand::Complete).await;
                end.reply = Err(reason);
                return end;
            }
        };
        self.settle(session, result, forwarded, end).await
    }

    /// Fill in `end` from the turn's `result` and hand the result to
    /// `session`.
    async fn settle(
        &self,
        session: &mut AgentSession,
        result: aura_agent::AgentLoopResult,
        forwarded: &ForwarderState,
        mut end: TaskEnd,
    ) -> TaskEnd {
        let text = result.final_text.clone();
        let llm_error = result.llm_error.as_ref().map(ToString::to_string);
        end.shown = shown_to_judge(result.messages.get(self.turn_start..).unwrap_or_default());
        end.files = result
            .file_changes
            .iter()
            .map(|change| change.path.clone())
            .collect();
        handle_agent_success(session, result, forwarded, self.ui, &[]).await;
        end.reply = match llm_error {
            _ if self.cancel.is_cancelled() => Err("cancelled by the user".to_string()),
            Some(err) if text.trim().is_empty() => Err(err),
            _ => Ok(text),
        };
        end
    }
}
