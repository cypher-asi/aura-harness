//! One `aura -p` run on the terminal's session machinery: the agent hub
//! and kernel factory open the session, turns run through the same
//! [`drive_turn`] as the terminal's, and the session is recorded and
//! checkpointed after every turn so `--resume` / `--continue` and the
//! terminal's `/resume` can pick it up.

use super::output::{ExitReason, Output, RunReport, StreamEvent, UsageReport};
use super::sink::{CallLimits, HeadlessSink, TurnTally};
use super::tools::ToolFilter;
use crate::event_loop::gate::followup::{
    self, ended_signal, failure_text, joined, takeover_prompt, At, Attempt, Before, CarryOn,
    FollowUp, Followed, Following, Judging, Takeover,
};
use crate::event_loop::handlers::{
    drive_turn, mark_session_start, record_response, resume_checkpoint,
};
use crate::event_loop::notices::{completion_notice, record_completion, Notice};
use crate::event_loop::processes::stop_background_commands;
use crate::event_loop::steering::{SteerSource, Steering};
use crate::event_loop::subagents::lock;
use crate::event_loop::turns::{drain_stale_inbox, record_prompt};
use crate::event_loop::{AgentHub, AgentSession};
use crate::model_selection::routed_effort;
use crate::session_checkpoint::SessionUsage;
use aura_agent::{AgentLoop, AgentLoopConfig, AgentLoopResult};
use aura_agent_kernel::Kernel;
use aura_agent_trace::RunRecorder;
use aura_core_modes::AgentMode;
use aura_core_types::{
    AgentId, OutcomePayload, OutcomeSignal, PromptCategory, PromptPayload, Route, RouteLevel,
    RouteReason, Transaction,
};
use aura_exec_tools::BackgroundProcessHook;
use aura_gate::attempts::Terms;
use aura_gate::evidence::outcome::calls_for_stronger;
use aura_gate::judge::criteria::Criteria;
use aura_gate::judge::turn::Failed;
use aura_gate::live::{self, Judged};
use aura_gate::route::{Decision, Work};
use aura_model::{Message, Role, ThinkingEffort};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

#[cfg(test)]
mod tests;

/// How long a turn may keep running after `--max-time` cancelled it
/// before it is dropped, losing the turn's unsaved steps.
const CANCEL_GRACE: Duration = Duration::from_secs(60);

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
    pub(crate) hub: Arc<AgentHub>,
    /// Every agent's background commands
    pub(crate) background: Arc<dyn BackgroundProcessHook>,
    /// Background commands that ended
    pub(crate) completions: mpsc::Receiver<Transaction>,
    /// Replies from `task` spawns and background agents
    pub(crate) notices: mpsc::UnboundedReceiver<Notice>,
    /// Model the turns run on, for the report
    pub(crate) model: String,
    /// The effort Gate chose for the turn, while that is Gate's to choose
    pub(crate) effort: Option<ThinkingEffort>,
    /// The effort the last turn's requests were held at, for a turn that
    /// carries on from it
    pub(crate) held_effort: Option<ThinkingEffort>,
    pub(crate) mode: AgentMode,
    pub(crate) workspace_root: PathBuf,
    pub(crate) tool_filter: ToolFilter,
    /// Longest wait for the agent's background work (commands, spawns)
    /// once it has nothing else to do
    pub(crate) job_wait: Duration,
    /// Run trace the turns are recorded to, for an eval task's run
    pub(crate) trace: Option<Arc<RunRecorder>>,
    /// Problems found while setting up that change what the run does,
    /// such as a plan's model list that would not load
    pub(crate) notes: Vec<String>,
    /// What Gate's judge found on the turns that failed, oldest first;
    /// whoever reports on the run takes them
    pub(crate) judged: Vec<Judged>,
    /// What the work on the run's prompt is checked against, written
    /// down before it started
    pub(crate) criteria: Option<Arc<Criteria>>,
    /// Whether the next prompt Gate classifies is the run's own, whose
    /// criteria are written, rather than a notice
    pub(crate) criteria_due: bool,
}

/// `<agent hex>:<session start ms>`: names one saved session.
pub(crate) fn session_id(agent_id: AgentId, started_ms: u64) -> String {
    format!("{}:{started_ms}", agent_id.to_hex())
}

/// The agent and session start named by `id`. A bare number names a
/// session of `default_agent`.
pub(crate) fn parse_session_id(id: &str, default_agent: AgentId) -> Result<(AgentId, u64), String> {
    let id = id.trim();
    let (agent, started) = match id.rsplit_once(':') {
        Some((agent, started)) => (
            AgentId::from_hex(agent).map_err(|e| format!("bad agent in session id `{id}`: {e}"))?,
            started,
        ),
        None => (default_agent, id),
    };
    let started = started
        .parse::<u64>()
        .map_err(|_| format!("`{id}` is not a session id (expected <agent>:<number>)"))?;
    Ok((agent, started))
}

/// How one agent turn ended.
struct TurnOutcome {
    /// How the turn went, when Gate routed it: the `Outcome` [`Headless::drive`]
    /// records once Gate's follow-up has had its say
    ended: Option<OutcomePayload>,
    result: Option<AgentLoopResult>,
    /// Model calls and tokens of the turn
    usage: SessionUsage,
    /// Set when the turn ended the run early
    stop: Option<(ExitReason, Option<String>)>,
    /// Notices the model never got, for the next turn
    unseen: Vec<String>,
    /// How the turn failed, in words, when Gate routed it and it did
    failure: Option<String>,
    /// Whether `stop` is a model error the turn's conversation survived,
    /// so a stronger model can still take the turn over
    can_go_on: bool,
    /// Where the turn's own messages start in `result`
    turn_start: usize,
    /// What the turn ran under, for Gate's attempt log
    terms: Terms,
    /// Whether nothing has said yet how the turn went, which leaves its
    /// `Outcome` for Gate's follow-up to settle
    unchecked: bool,
}

impl TurnOutcome {
    /// A turn that never ran: `problem` ends the run, with the session's
    /// messages ending at `turn_start`.
    fn stopped(problem: String, turn_start: usize) -> Self {
        Self {
            ended: None,
            result: None,
            usage: SessionUsage::default(),
            stop: Some((ExitReason::Error, Some(problem))),
            unseen: Vec::new(),
            failure: None,
            can_go_on: false,
            turn_start,
            terms: Terms::default(),
            unchecked: false,
        }
    }
}

/// What a turn carries on from the one before it, which ran under the
/// `Terms` each carries.
enum Carry {
    /// Work a weaker model failed at, on this model, with the work's
    /// category; it keeps the work's difficulty.
    Escalate(Decision, PromptCategory, Terms),
    /// The same model, asked to check the change of the turn that ended
    /// as this; the new turn's `Outcome` takes its place, and it is done
    /// the same way, of the same difficulty.
    Nudge(OutcomePayload, Terms),
}

/// Whether the agent ended `result` on its own: a final reply with no
/// call left unanswered and no model error.
fn finished_naturally(result: &AgentLoopResult) -> bool {
    result.llm_error.is_none()
        && result
            .messages
            .last()
            .is_some_and(|m| m.role == Role::Assistant && !m.has_tool_use())
}

/// What a run has accumulated so far.
#[derive(Default)]
struct Totals {
    usage: SessionUsage,
    agent_turns: u32,
    compactions: u32,
    files_changed: Vec<String>,
    reply: String,
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
            let label = |id: &str| aura_model::model_catalog::route_label(id);
            let classifier = self.hub.classifier();
            lines.push(format!(
                "gate: prompts classified by {}",
                if classifier.is_empty() {
                    "nothing".to_string()
                } else {
                    label(&classifier)
                }
            ));
            let models: Vec<String> = candidates.iter().map(|c| label(&c.id)).collect();
            lines.push(format!("gate: routing among {}", models.join(", ")));
        }
        lines
    }

    /// Run `request` to the end, streaming to `output`. `interrupt` stops
    /// the run (Ctrl+C).
    pub(crate) async fn execute(
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
        let session_id = session_id(session.agent_id(), session.started_ms);
        self.announce(output, &session, &session_id, resumed);
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
    /// turn, Gate follows it up first ([`Self::follow_up`]): a turn that
    /// ran no check is asked for one, and work that is hard or shows a
    /// sign of failure is reviewed before it stands. Each routed turn's
    /// `Outcome` and attempt line are written once, after its follow-up,
    /// unless its model carries on with the work. When it ends in a model
    /// error, with its last check or the harness's failing, or with its
    /// unchecked work judged unfinished, Gate's judge says whether the
    /// model was at fault ([`Self::judge`]). If it was, or nothing judged
    /// the turn, it goes on in the same conversation on the next stronger
    /// model, tier by tier. Also returns the `Outcome` recorded for the
    /// last of those turns, when Gate routed them.
    pub(super) async fn drive(
        &mut self,
        session: &mut AgentSession,
        prompt: String,
        limits: RunLimits,
        started: Instant,
        output: &Output,
        interrupt: &CancellationToken,
    ) -> (RunReport, Option<OutcomePayload>) {
        let deadline = limits.max_time.map(|limit| started + limit);
        let session_id = session_id(session.agent_id(), session.started_ms);
        let started_ms = crate::event_loop::processes::now_ms();
        let mut totals = Totals::default();
        // Watched from here so the judge can see what a failed turn
        // changed.
        let files = session.file_checkpoints.clone();
        if let Some(files) = &files {
            files.begin_attempt(session.agent_id());
        }
        // The workspace as it is now, so a review sees everything the
        // turns change, by their commands as well as their file tools.
        let settings = *self.hub.gate.settings();
        self.criteria = None;
        self.criteria_due = true;
        let reviews = (settings.review != aura_config::ReviewMode::Off
            || settings.criteria != aura_config::ReviewMode::Off)
            && settings.judge
            && self
                .hub
                .gate_candidates(session.definition.as_deref())
                .is_some_and(|candidates| !candidates.is_empty());
        let before = if reviews {
            Before::of_workspace(session.kernel.agent_workspace()).await
        } else {
            None
        };
        let task = prompt.clone();
        let mut prompt = prompt;
        let mut first_ended = None;
        let mut stop = None;
        // Whether the turns so far answer `prompt` itself (its first turn
        // and the stronger models it went on to) rather than a notice.
        let mut answering = true;
        let mut carry = None;
        // The follow-up of the attempt so far, and where that attempt's
        // messages start.
        let mut follow = Following::default();
        let mut attempt_start = 0;
        loop {
            totals.agent_turns += 1;
            let nudged = matches!(carry, Some(Carry::Nudge(..)));
            let mut outcome = self
                .run_turn(
                    session,
                    prompt,
                    limits,
                    deadline,
                    totals.usage.clone(),
                    carry.take(),
                    output,
                    interrupt,
                )
                .await;
            if !nudged {
                attempt_start = outcome.turn_start;
            }
            let mut found = Followed::default();
            if answering && outcome.stop.is_none() {
                found = self
                    .follow_up(
                        session,
                        &task,
                        &mut outcome,
                        (&mut follow, before.as_ref()),
                        attempt_start,
                        output,
                    )
                    .await;
                if found.harness.is_some() {
                    outcome.failure.clone_from(&found.harness);
                }
            }
            // The turn's outcome stands once the follow-up has had its say,
            // unless its model carries on with the work.
            if found.carry_on.is_none() {
                if let Some(ended) = &outcome.ended {
                    let left = (outcome.result.as_ref())
                        .map(|result| follow.steps.evidence(result, attempt_start));
                    let gate = &self.hub.gate;
                    gate.log_attempt(session.agent_id(), ended, outcome.terms, left);
                    live::record(&session.kernel, session.agent_id(), ended).await;
                }
            }
            // What the turn's signal said, and what the judge found wrong
            // with it.
            let raw = outcome.ended.as_ref().map(|ended| ended.signal);
            let mut review = found.wrong.take();
            if answering && found.carry_on.is_none() && review.is_none() {
                let judged = self
                    .judge(session, &task, &outcome, &mut follow.steps, output)
                    .await;
                if let Some((judged_as, judged)) = judged {
                    outcome.ended = Some(judged_as);
                    review = Some(judged.verdict.reasons.clone());
                    self.judged.push(judged);
                }
            }
            if answering {
                first_ended.clone_from(&outcome.ended);
            }
            totals.usage.add(&outcome.usage);
            if let Some(result) = &outcome.result {
                totals.compactions += result.summary_compactions;
                for change in &result.file_changes {
                    if !totals.files_changed.contains(&change.path) {
                        totals.files_changed.push(change.path.clone());
                    }
                }
                if !result.total_text.trim().is_empty() {
                    totals.reply.clone_from(&result.total_text);
                }
            }
            let carried = found
                .carry_on
                .take()
                .and_then(|carry_on| Some((carry_on, outcome.ended.take()?)));
            if let Some((
                CarryOn {
                    message, notice, ..
                },
                ended,
            )) = carried
            {
                output.event(&StreamEvent::Notice { text: &notice });
                let mut next = message;
                for unseen in &outcome.unseen {
                    next.push_str("\n\n");
                    next.push_str(unseen);
                }
                prompt = next;
                carry = Some(Carry::Nudge(ended, outcome.terms));
                continue;
            }
            if answering && (outcome.stop.is_none() || outcome.can_go_on) {
                if let Some((decision, ended)) = self.stronger(session, outcome.ended.as_ref()) {
                    let review = joined(found.harness.take(), review);
                    let signal = raw.unwrap_or(ended.signal);
                    let takeover = Takeover::new(&ended.model, decision, signal, review.as_deref());
                    let mut next = takeover.text;
                    for unseen in &outcome.unseen {
                        next.push_str("\n\n");
                        next.push_str(unseen);
                    }
                    output.event(&StreamEvent::Notice { text: &next });
                    prompt = format!("{}{next}", SteerSource::Harness.prefix());
                    carry = Some(Carry::Escalate(
                        takeover.next,
                        ended.category,
                        outcome.terms,
                    ));
                    continue;
                }
            }
            if outcome.stop.is_some() {
                stop = outcome.stop;
                break;
            }
            answering = false;
            let next = if outcome.unseen.is_empty() {
                match self
                    .next_notice(&session.kernel, deadline, interrupt, output)
                    .await
                {
                    Ok(Some(notice)) => notice,
                    Ok(None) => break,
                    Err(reason) => {
                        stop = Some((reason, None));
                        break;
                    }
                }
            } else {
                outcome.unseen.join("\n\n")
            };
            output.event(&StreamEvent::Notice { text: &next });
            prompt = format!("{}{next}", SteerSource::Harness.prefix());
        }
        stop_background_commands(self.background.as_ref());
        if let Some(files) = &files {
            let _ = files.take_attempt(session.agent_id());
        }

        let (exit_reason, error) = stop.unwrap_or((ExitReason::Success, None));
        let usage = UsageReport::from(&totals.usage);
        let gate_cost_usd = self.hub.gate_cost_usd(session.agent_id(), started_ms);
        let report = RunReport {
            kind: "result",
            exit_reason,
            is_error: exit_reason != ExitReason::Success,
            result: totals.reply,
            session_id,
            model: self.model.clone(),
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
            todos: session.todos.clone(),
            files_changed: totals.files_changed,
            error,
        };
        (report, first_ended)
    }

    /// Ask Gate's judge about `turn`, a turn of `session` answering
    /// `task`, when Gate routed it and it failed on a model weaker than
    /// the judge: was the model at fault? The finding is streamed to
    /// `output` and recorded as a later `Outcome` of the turn's prompt,
    /// which is returned with it: `judged_fail` counts against the model
    /// and sends the turn on to a stronger one, `judged_excused` counts
    /// for nothing and leaves the turn where it is. `None` when the turn
    /// is not judged or the judge gave no verdict; the turn's own signal
    /// then stands.
    ///
    /// The judge is only asked about failures, so what it costs is paid
    /// when there is something to learn; the cost is reported on its own
    /// ([`Judged::cost_usd`]), not as part of the work.
    async fn judge(
        &self,
        session: &AgentSession,
        task: &str,
        turn: &TurnOutcome,
        follow: &mut FollowUp,
        output: &Output,
    ) -> Option<(OutcomePayload, Judged)> {
        let ended = turn.ended.as_ref()?;
        let judge = self.hub.judge_of(session.definition.as_deref(), ended)?;
        let diff = follow.diff(session, turn.result.as_ref()).await;
        let workspace = session.kernel.agent_workspace();
        let (config, _) = self.hub.turn(session.definition.as_deref(), &workspace);
        let judging = Judging {
            session,
            judge: &judge,
            ended,
            shown: Failed {
                task,
                diff: &diff,
                reply: turn.result.as_ref().map_or("", |r| r.total_text.as_str()),
                failure: turn.failure.as_deref().unwrap_or_default(),
            },
            auth_token: config.auth_token,
        };
        judging.ask(output).await
    }

    /// Follow up `turn`, a turn of `session` answering `task` that Gate
    /// routed, as Gate's settings allow (see
    /// [`gate::followup`](crate::event_loop::gate::followup)): one that ran
    /// no check of its own is asked once to check its change, else the
    /// harness runs the project's tests, else the judge looks at work
    /// that seems unfinished; and work that is hard or shows a sign of
    /// failure is reviewed before it stands, going back to its model once
    /// with what the review finds. What the harness's check, the judge or
    /// the review finds becomes `turn`'s `Outcome`. `follow` holds the
    /// steps the attempt has taken, `before` the workspace as it was when
    /// the run began, and `since` is where that attempt's messages start.
    async fn follow_up(
        &mut self,
        session: &AgentSession,
        task: &str,
        turn: &mut TurnOutcome,
        (follow, before): (&mut Following, Option<&Before>),
        since: usize,
        output: &Output,
    ) -> Followed {
        let (Some(ended), Some(result)) = (turn.ended.as_mut(), turn.result.as_ref()) else {
            return Followed::default();
        };
        let Some(candidates) = self.hub.gate_candidates(session.definition.as_deref()) else {
            return Followed::default();
        };
        let workspace = session.kernel.agent_workspace();
        let (config, _) = self.hub.turn(session.definition.as_deref(), &workspace);
        let signal = (!turn.unchecked).then_some(ended.signal);
        let attempt = Attempt {
            session,
            live: &self.hub.gate,
            candidates: &candidates,
            task,
            ran: (ended.model.as_str(), self.effort),
            result,
            at: At {
                category: ended.category,
                difficulty: turn.terms.difficulty,
                signal,
                turn_start: since,
            },
            prompt_tx: Some(ended.prompt_tx),
            workspace: &workspace,
            before,
            criteria: self.criteria.as_ref(),
            auth_token: config.auth_token,
        };
        let mut followed = follow.run(&attempt, output).await;
        if let Some(found) = followed.signal.filter(|found| Some(*found) != signal) {
            *ended = ended.with_signal(found);
            turn.unchecked = false;
        }
        self.judged.append(&mut followed.judged);
        followed
    }

    /// Where a turn of `session` that Gate routed and that `ended` badly
    /// goes on: the next stronger model, with how the turn ended. `None`
    /// when it ended well enough or nothing is stronger.
    fn stronger<'a>(
        &self,
        session: &AgentSession,
        ended: Option<&'a OutcomePayload>,
    ) -> Option<(Decision, &'a OutcomePayload)> {
        let ended = ended.filter(|ended| calls_for_stronger(ended.signal))?;
        let candidates = self.hub.gate_candidates(session.definition.as_deref())?;
        let ran = (ended.model.as_str(), self.effort);
        let decision = followup::stronger(
            &self.hub.gate,
            &candidates,
            ended.category,
            ran,
            ended.signal,
        )?;
        info!(
            from = %ended.model,
            to = %decision.model.id,
            signal = %ended.signal,
            "Gate moves the turn to a stronger model"
        );
        Some((decision, ended))
    }

    /// Open the session `choice` names. Returns it and whether it
    /// continues an earlier one.
    async fn open(&self, choice: &SessionChoice) -> Result<(AgentSession, bool), String> {
        let headless = lock(&self.hub.registry)
            .headless()
            .map_err(|e| format!("registering the headless agent: {e:#}"))?;
        let (agent, started_ms) = match choice {
            SessionChoice::New | SessionChoice::Continue => (headless, None),
            SessionChoice::Resume(id) => {
                let (agent_id, started_ms) = parse_session_id(id, headless.agent_id())?;
                let agent = lock(&self.hub.registry)
                    .find(agent_id)
                    .cloned()
                    .ok_or_else(|| format!("no agent for session {id}"))?;
                (agent, Some(started_ms))
            }
        };
        let folder = lock(&self.hub.registry).project_folder(agent.agent_id());
        let mut session = self
            .hub
            .factory
            .open(&agent, Vec::new(), folder)
            .map_err(|e| format!("opening {}: {e:#}", agent.name))?;

        match (choice, started_ms) {
            (SessionChoice::New, _) => {
                if !session.messages.is_empty() {
                    mark_session_start(&session).await;
                }
                session.reset();
                Ok((session, false))
            }
            (SessionChoice::Resume(id), Some(started_ms)) if started_ms != session.started_ms => {
                let checkpoint = session
                    .checkpoints
                    .as_ref()
                    .and_then(|c| c.load(session.agent_id(), started_ms))
                    .ok_or_else(|| format!("no saved session {id}"))?;
                info!(session = %id, "Resuming a saved session");
                resume_checkpoint(&mut session, checkpoint).await;
                Ok((session, true))
            }
            _ => {
                let resumed = !session.messages.is_empty();
                Ok((session, resumed))
            }
        }
    }

    fn announce(&self, output: &Output, session: &AgentSession, session_id: &str, resumed: bool) {
        let workspace = session.kernel.agent_workspace();
        // A turn that leads has fewer tools; which turns do is only known
        // once their prompt is classified.
        let (_, tools) = self.hub.turn(session.definition.as_deref(), &workspace);
        let tools = self.tool_filter.filter(tools);
        let agent_hex = session.agent_id().to_hex();
        let cwd = self.workspace_root.display().to_string();
        let mode = format!("{:?}", self.mode).to_lowercase();
        output.event(&StreamEvent::Init {
            session_id,
            agent_id: &agent_hex,
            model: &self.model,
            cwd: &cwd,
            mode: &mode,
            tools: tools.iter().map(|t| t.name.as_str()).collect(),
            resumed,
        });
    }

    /// Record `text` as the prompt and run one agent turn on `session`,
    /// with `before` the usage of the run's earlier turns. When Gate
    /// routes the turn, it ends with an `Outcome` saying how it went.
    /// `carry` is set for a turn that goes on from the one before: with
    /// work a weaker model failed at, or on the same model asked to check
    /// its change, which the `Outcome` then speaks for, its cost included.
    /// Either keeps the difficulty the classifier gave the work.
    #[allow(clippy::too_many_arguments)]
    async fn run_turn(
        &mut self,
        session: &mut AgentSession,
        text: String,
        limits: RunLimits,
        deadline: Option<Instant>,
        before: SessionUsage,
        carry: Option<Carry>,
        output: &Output,
        interrupt: &CancellationToken,
    ) -> TurnOutcome {
        drain_stale_inbox(session).await;
        let started = Instant::now();
        let workspace = session.kernel.agent_workspace();
        let (mut config, mut tools, lead) = self.hub.user_turn(session);
        if let Some(max) = limits.max_iterations {
            config.max_iterations = max;
        }
        let carried = carry.is_some();
        let (escalation, nudged, earlier) = match carry {
            Some(Carry::Escalate(decision, category, terms)) => {
                (Some((decision, category)), None, Some(terms))
            }
            Some(Carry::Nudge(ended, terms)) => (None, Some(ended), Some(terms)),
            None => (None, None, None),
        };
        let routing = match &nudged {
            Some(ended) => {
                if config.model != ended.model {
                    self.hub.model_limits().configure(&mut config, &ended.model);
                }
                Ok((PromptPayload::new(text.clone()), Some(ended.category), None))
            }
            None => {
                self.route_turn(session, &mut config, &text, escalation)
                    .await
            }
        };
        let (mut payload, category, told) = match routing {
            Ok(routing) => routing,
            Err(problem) => return TurnOutcome::stopped(problem, session.messages.len()),
        };
        let routed = category.is_some();
        let difficulty = payload
            .difficulty
            .or(earlier.and_then(|terms| terms.difficulty));
        if let (Some(lead), Some(category)) = (lead, category) {
            let explore = payload.route.reason != RouteReason::Escalate && nudged.is_none();
            let kept = earlier
                .filter(|_| nudged.is_some())
                .and_then(|terms| terms.led);
            let led = kept.unwrap_or_else(|| {
                self.hub
                    .gate
                    .way((category, difficulty), &config.model, explore)
                    .led
            });
            session.lead = lead.apply(led, &mut config, &mut tools);
            payload.led = Some(led);
        }
        let terms = Terms {
            difficulty,
            led: payload.led,
            routed,
        };
        let started_ms = crate::event_loop::processes::now_ms();
        let prompt_tx = match record_prompt(session, &payload).await {
            Ok((tx, _)) => tx.hash,
            Err(problem) => return TurnOutcome::stopped(problem, session.messages.len()),
        };
        if let Some(trace) = &self.trace {
            config.trace_blobs = trace.blob_store();
            trace.begin_turn(&text);
        }
        let turn_start = session.messages.len();
        session
            .messages
            .push(Message::user(format!("{text}{}", told.unwrap_or_default())));
        self.hub.register_root(
            session.agent_id(),
            &session.identity.name,
            workspace,
            &config,
        );
        config.initial_todos.clone_from(&session.todos);
        if carried {
            config.initial_thinking_effort = self.held_effort;
        }
        self.held_effort = config.held_thinking_effort();
        let context_limit = config.max_context_tokens;
        let agent_loop = AgentLoop::new(config);
        let tools = self.tool_filter.filter(tools);

        let turn_cancel = interrupt.child_token();
        let steering = Steering::new(&turn_cancel);
        let (event_tx, event_rx) = mpsc::channel(1024);
        let sink = HeadlessSink::new(
            output.clone(),
            before,
            CallLimits {
                max_calls: limits.max_turns,
                max_cost_usd: limits.max_cost_usd,
            },
            turn_cancel.clone(),
            context_limit,
        )
        .with_trace(self.trace.clone());
        let forwarder = tokio::spawn(sink.forward(event_rx));

        // Cancelling at the deadline lets the loop stop between steps and
        // keep what it did; the timeout below is only a backstop.
        let out_of_time = Arc::new(AtomicBool::new(false));
        let watcher = deadline.map(|deadline| {
            let out_of_time = Arc::clone(&out_of_time);
            let cancel = turn_cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep_until(deadline.into()).await;
                out_of_time.store(true, Ordering::Release);
                cancel.cancel();
            })
        });
        let backstop = deadline.map(|d| d.saturating_duration_since(Instant::now()) + CANCEL_GRACE);

        let kernel = Arc::clone(&session.kernel);
        // Listed as running so the subtasks the turn dispatches are told
        // apart from those of the next turn.
        self.hub.start_running(
            kernel.agent_id,
            Arc::clone(&kernel),
            turn_cancel.clone(),
            None,
        );
        let mut late = Vec::new();
        let result = {
            let turn = drive_turn(
                session,
                &agent_loop,
                tools,
                event_tx,
                &turn_cancel,
                &steering,
                backstop,
            );
            tokio::pin!(turn);
            loop {
                tokio::select! {
                    result = &mut turn => break result,
                    Some(tx) = self.completions.recv() => {
                        if let Some(notice) = background_notice(&self.hub, &kernel, tx).await {
                            steer_or_keep(&steering, notice, &mut late).await;
                        }
                    }
                    Some(notice) = self.notices.recv() => {
                        if notice.to == kernel.agent_id {
                            steer_or_keep(&steering, notice, &mut late).await;
                        }
                    }
                }
            }
        };
        self.hub.stop_running(kernel.agent_id);
        if let Some(watcher) = watcher {
            watcher.abort();
        }
        let tally = forwarder.await.unwrap_or_else(|e| {
            warn!(error = %e, "Headless event sink panicked");
            TurnTally::default()
        });
        session.usage.add(&tally.usage);
        if let Some(trace) = &self.trace {
            match result.as_ref() {
                Some(Ok(result)) => trace.end_turn(result),
                failed => trace.end_turn(&AgentLoopResult {
                    llm_error: Some(failed.and_then(|r| r.as_ref().err()).map_or_else(
                        || "the turn did not stop in time".to_string(),
                        ToString::to_string,
                    )),
                    ..AgentLoopResult::default()
                }),
            }
        }

        let limit = if interrupt.is_cancelled() {
            Some(ExitReason::Interrupted)
        } else if out_of_time.load(Ordering::Acquire) {
            Some(ExitReason::MaxTime)
        } else {
            tally.limit_hit
        };
        let messages = result
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .map_or(&[][..], |r| r.messages.as_slice());
        let (seen, unseen) = steering.finish(messages);
        let mut unseen: Vec<String> = unseen.into_iter().map(|(_, text)| text).collect();
        unseen.append(&mut late);
        let mut failure = None;
        let mut unchecked = false;
        let ended = if let Some(category) = category {
            // A run stopped by its own limits says nothing about the model.
            let own = match (&result, limit) {
                (Some(result), None) => {
                    failure = failure_text(result, turn_start);
                    ended_signal(result, turn_start)
                }
                _ => Some(OutcomeSignal::Cancelled),
            };
            unchecked = own.is_none();
            let signal = own.unwrap_or(OutcomeSignal::Unavailable);
            let mut outcome = OutcomePayload {
                prompt_tx,
                category,
                model: agent_loop.config().model.clone(),
                success: signal.success(),
                signal,
                cost_usd: tally.usage.cost_usd,
                subtask_cost_usd: self.hub.subtask_cost_usd(session.agent_id(), started_ms),
                latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            };
            if let Some(before) = &nudged {
                outcome.prompt_tx = before.prompt_tx;
                outcome.cost_usd += before.cost_usd;
                outcome.subtask_cost_usd += before.subtask_cost_usd;
                outcome.latency_ms = outcome.latency_ms.saturating_add(before.latency_ms);
            }
            Some(outcome)
        } else {
            None
        };

        let mut outcome = TurnOutcome {
            ended,
            result: None,
            usage: tally.usage,
            stop: None,
            unseen,
            failure,
            can_go_on: false,
            turn_start,
            terms,
            unchecked,
        };
        match result {
            Some(Ok(result)) => {
                commit(session, &result, &seen).await;
                outcome.stop = turn_stop(limit, &result);
                // The model's call failed, not the run: its conversation
                // is kept, so another model can go on from it.
                outcome.can_go_on =
                    limit.is_none() && !result.insufficient_credits && result.llm_error.is_some();
                outcome.result = Some(result);
            }
            Some(Err(e)) => {
                let message = e.to_string();
                outcome.stop = Some((limit.unwrap_or(ExitReason::Error), Some(message)));
            }
            None => {
                outcome.stop = Some((
                    ExitReason::MaxTime,
                    Some("the turn did not stop in time after --max-time".to_string()),
                ));
            }
        }
        outcome
    }

    /// The prompt record of a turn of `session` answering `text`, and the
    /// category of its work when Gate routed it. Gate routes the turn when
    /// the user selected gate, the agent names no model of its own and
    /// there is a model to route to: the prompt is classified and `config`
    /// pointed at Gate's pick, which the subtasks the turn dispatches are
    /// routed by. Otherwise the prompt is recorded as it is. With
    /// `escalation`, the turn goes to that model with that category,
    /// recorded with route reason `escalate`.
    ///
    /// # Errors
    /// Says why when Gate routes the turn and could not classify it.
    async fn route_turn(
        &mut self,
        session: &AgentSession,
        config: &mut AgentLoopConfig,
        text: &str,
        escalation: Option<(Decision, PromptCategory)>,
    ) -> Result<(PromptPayload, Option<PromptCategory>, Option<String>), String> {
        if let Some((decision, category)) = escalation {
            self.point(session, config, &decision);
            let payload = takeover_prompt(text.to_string(), category, &decision, String::new());
            return Ok((payload, Some(category), None));
        }
        let Some(candidates) = self
            .hub
            .gate_candidates(session.definition.as_deref())
            .filter(|candidates| !candidates.is_empty())
        else {
            return Ok((PromptPayload::new(text), None, None));
        };
        let classifier = self.hub.classifier();
        if classifier.is_empty() {
            return Err("Gate has no model to classify the prompt".to_string());
        }
        let label = live::classify_prompt(
            &session.kernel,
            &classifier,
            text,
            config.auth_token.clone(),
        )
        .await?;
        let category = label.category;
        let mut difficulty = Some(label.difficulty);
        let mut told = None;
        if std::mem::take(&mut self.criteria_due) {
            self.criteria = live::hold_to_criteria(
                &session.kernel,
                (&self.hub.gate, &candidates),
                category,
                &mut difficulty,
                text,
                config.auth_token.clone(),
            )
            .await
            .ok()
            .flatten();
            told = live::worker_note_for(self.criteria.as_deref());
        }
        let route = match self
            .hub
            .gate
            .decide(&Work::turn(category).of(difficulty), &candidates)
        {
            Some(decision) => {
                info!(
                    model = %decision.model.id,
                    %category,
                    reason = %decision.reason,
                    "Gate routed the turn"
                );
                self.point(session, config, &decision);
                decision.route(RouteLevel::Turn)
            }
            None => Route::turn(config.model.clone(), RouteReason::Default),
        };
        let payload = PromptPayload {
            category: Some(category),
            classifier,
            route,
            difficulty,
            ..PromptPayload::new(text)
        };
        Ok((payload, Some(category), told))
    }

    /// Point `config`, of a turn of `session`, at `decision`'s model and
    /// effort.
    fn point(&mut self, session: &AgentSession, config: &mut AgentLoopConfig, decision: &Decision) {
        self.hub.model_limits().route(config, decision);
        self.model.clone_from(&decision.model.id);
        self.effort = routed_effort(config);
        self.hub
            .gate
            .note_model(session.agent_id(), &decision.model.id);
    }

    /// Wait for the next notice for `kernel`'s agent: a background
    /// command ended, or a spawn replied. `Ok(None)` when it has no
    /// background work left, or the work outlasts `job_wait` (commands are
    /// then stopped); `Err` when the run's deadline passes or it is
    /// interrupted while waiting.
    async fn next_notice(
        &mut self,
        kernel: &Kernel,
        deadline: Option<Instant>,
        interrupt: &CancellationToken,
        output: &Output,
    ) -> Result<Option<String>, ExitReason> {
        let agent_id = kernel.agent_id;
        while let Ok(tx) = self.completions.try_recv() {
            if let Some(notice) = background_notice(&self.hub, kernel, tx).await {
                return Ok(Some(notice.prompt));
            }
        }
        while let Ok(notice) = self.notices.try_recv() {
            if notice.to == agent_id {
                return Ok(Some(notice.prompt));
            }
        }
        if !self.hub.processes.has_background_work(agent_id) {
            return Ok(None);
        }
        let wait_until = Instant::now() + self.job_wait;
        let until = deadline.map_or(wait_until, |d| d.min(wait_until));
        loop {
            tokio::select! {
                () = interrupt.cancelled() => return Err(ExitReason::Interrupted),
                () = tokio::time::sleep_until(until.into()) => {
                    if deadline.is_some_and(|d| d <= until) {
                        return Err(ExitReason::MaxTime);
                    }
                    let message = format!(
                        "Stopping background jobs still running after {}s",
                        self.job_wait.as_secs()
                    );
                    warn!("{message}");
                    output.event(&StreamEvent::Warning { message: &message });
                    return Ok(None);
                }
                tx = self.completions.recv() => match tx {
                    Some(tx) => {
                        if let Some(notice) = background_notice(&self.hub, kernel, tx).await {
                            return Ok(Some(notice.prompt));
                        }
                    }
                    None => return Ok(None),
                },
                Some(notice) = self.notices.recv() => {
                    if notice.to == agent_id {
                        return Ok(Some(notice.prompt));
                    }
                }
            }
        }
    }
}

/// Record the background command completion `tx` on `kernel` and return
/// the notice for its agent, when it is `kernel`'s agent and it should be
/// told.
async fn background_notice(hub: &AgentHub, kernel: &Kernel, tx: Transaction) -> Option<Notice> {
    let notice = completion_notice(&hub.processes, &tx);
    if tx.agent_id != kernel.agent_id {
        debug!(agent = %tx.agent_id, "Background command of another agent ended");
        return None;
    }
    record_completion(kernel, &tx).await;
    notice
}

/// Give `notice` to the running turn, or keep it in `late` for the next
/// one when the turn no longer takes input.
async fn steer_or_keep(steering: &Steering, notice: Notice, late: &mut Vec<String>) {
    if !steering
        .send(
            SteerSource::Harness,
            notice.prompt.clone(),
            notice.prompt.clone(),
        )
        .await
    {
        late.push(notice.prompt);
    }
}

/// Why the run stops after a turn that returned `result`, if it does.
fn turn_stop(
    limit: Option<ExitReason>,
    result: &AgentLoopResult,
) -> Option<(ExitReason, Option<String>)> {
    match limit {
        // A limit that hit as the agent wrote its final reply stopped
        // nothing.
        Some(ExitReason::Interrupted) => Some((ExitReason::Interrupted, None)),
        Some(reason) if !finished_naturally(result) => Some((reason, None)),
        _ if result.insufficient_credits => {
            Some((ExitReason::Error, Some("insufficient credits".to_string())))
        }
        _ => match &result.llm_error {
            Some(err) if result.total_text.trim().is_empty() => {
                Some((ExitReason::Error, Some(err.clone())))
            }
            _ => None,
        },
    }
}

/// Keep what a turn produced: its conversation, todo list, the notices
/// the model got mid-turn, and its reply, then checkpoint the session.
async fn commit(session: &mut AgentSession, result: &AgentLoopResult, seen: &[String]) {
    session.messages.clone_from(&result.messages);
    session.todos.clone_from(&result.todos);
    for text in seen {
        if let Err(problem) = record_prompt(session, &PromptPayload::new(text.clone())).await {
            warn!(%problem, "Failed to record a mid-turn notice");
        }
    }
    record_response(session, &result.total_text).await;
    session.save_checkpoint().await;
}
