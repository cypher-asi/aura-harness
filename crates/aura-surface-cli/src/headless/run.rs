//! One `aura -p` run on the terminal's session machinery: the agent hub
//! and kernel factory open the session, turns run through the same
//! [`drive_turn`] as the terminal's, and the session is recorded and
//! checkpointed after every turn so `--resume` / `--continue` and the
//! terminal's `/resume` can pick it up.

use super::output::{ExitReason, Output, RunReport, StreamEvent, UsageReport};
use super::sink::{CallLimits, HeadlessSink, TurnTally};
use super::tools::ToolFilter;
use crate::event_loop::handlers::{
    drive_turn, mark_session_start, record_response, resume_checkpoint,
};
use crate::event_loop::notices::{completion_notice, record_completion, Notice};
use crate::event_loop::processes::stop_background_commands;
use crate::event_loop::steering::{SteerSource, Steering};
use crate::event_loop::subagents::lock;
use crate::event_loop::turns::{drain_stale_inbox, record_prompt};
use crate::event_loop::{AgentHub, AgentSession};
use crate::session_checkpoint::SessionUsage;
use aura_agent::{AgentLoop, AgentLoopResult};
use aura_agent_kernel::Kernel;
use aura_core_modes::AgentMode;
use aura_core_types::{AgentId, Transaction};
use aura_model_reasoner::{Message, Role};
use aura_tools::BackgroundProcessHook;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

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
}

pub(crate) struct Request {
    pub prompt: String,
    pub session: SessionChoice,
    pub limits: RunLimits,
}

/// The parts of a headless run that outlive one request.
pub(crate) struct Headless {
    pub hub: Arc<AgentHub>,
    /// Every agent's background commands
    pub background: Arc<dyn BackgroundProcessHook>,
    /// Background commands that ended
    pub completions: mpsc::Receiver<Transaction>,
    /// Replies from `task` spawns and background agents
    pub notices: mpsc::UnboundedReceiver<Notice>,
    /// Model the turns run on, for the report
    pub model: String,
    pub mode: AgentMode,
    pub workspace_root: PathBuf,
    pub tool_filter: ToolFilter,
    /// Longest wait for the agent's background work (commands, spawns)
    /// once it has nothing else to do
    pub job_wait: Duration,
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
    result: Option<AgentLoopResult>,
    /// Model calls and tokens of the turn
    usage: SessionUsage,
    /// Set when the turn ended the run early
    stop: Option<(ExitReason, Option<String>)>,
    /// Notices the model never got, for the next turn
    unseen: Vec<String>,
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
    /// Run `request` to the end, streaming to `output`. `interrupt` stops
    /// the run (Ctrl+C).
    pub(crate) async fn execute(
        &mut self,
        request: Request,
        output: &Output,
        interrupt: &CancellationToken,
    ) -> RunReport {
        let started = Instant::now();
        let deadline = request.limits.max_time.map(|limit| started + limit);
        let (mut session, resumed) = match self.open(&request.session).await {
            Ok(opened) => opened,
            Err(problem) => return RunReport::failed(problem, self.model.clone()),
        };
        let agent_id = session.agent_id();
        let session_id = session_id(agent_id, session.started_ms);
        self.announce(output, &session, &session_id, resumed);

        let mut totals = Totals::default();
        let mut prompt = request.prompt;
        let mut stop = None;
        loop {
            totals.agent_turns += 1;
            let outcome = self
                .run_turn(
                    &mut session,
                    prompt,
                    request.limits,
                    deadline,
                    totals.usage,
                    output,
                    interrupt,
                )
                .await;
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
            if outcome.stop.is_some() {
                stop = outcome.stop;
                break;
            }
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

        let (exit_reason, error) = stop.unwrap_or((ExitReason::Success, None));
        let usage = UsageReport::from(&totals.usage);
        RunReport {
            kind: "result",
            exit_reason,
            is_error: exit_reason != ExitReason::Success,
            result: totals.reply,
            session_id,
            model: self.model.clone(),
            num_turns: totals.usage.calls,
            agent_turns: totals.agent_turns,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            total_cost_usd: usage.cost_usd,
            usage,
            compactions: totals.compactions,
            todos: session.todos.clone(),
            files_changed: totals.files_changed,
            error,
        }
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
    /// with `before` the usage of the run's earlier turns.
    #[allow(clippy::too_many_arguments)]
    async fn run_turn(
        &mut self,
        session: &mut AgentSession,
        text: String,
        limits: RunLimits,
        deadline: Option<Instant>,
        before: SessionUsage,
        output: &Output,
        interrupt: &CancellationToken,
    ) -> TurnOutcome {
        drain_stale_inbox(session).await;
        if let Err(problem) = record_prompt(session, text.clone()).await {
            return TurnOutcome {
                result: None,
                usage: SessionUsage::default(),
                stop: Some((ExitReason::Error, Some(problem))),
                unseen: Vec::new(),
            };
        }
        session.messages.push(Message::user(text.clone()));

        let workspace = session.kernel.agent_workspace();
        let (mut config, tools) = self.hub.agent_turn(
            session.agent_id(),
            session.definition.as_deref(),
            &workspace,
        );
        self.hub.register_root(
            session.agent_id(),
            &session.identity.name,
            workspace,
            &config,
        );
        config.initial_todos.clone_from(&session.todos);
        let context_limit = config.max_context_tokens;
        let agent_loop = AgentLoop::new(config);
        let tools = self.tool_filter.filter(tools);
        if let Some(runner) = self.hub.gate_drafter(&tools) {
            if let Some(draft) = crate::gate::draft(&runner, &session.kernel, &text).await {
                session.messages.extend(draft);
            }
        }

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
        );
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
        if let Some(watcher) = watcher {
            watcher.abort();
        }
        let tally = forwarder.await.unwrap_or_else(|e| {
            warn!(error = %e, "Headless event sink panicked");
            TurnTally::default()
        });
        session.usage.add(&tally.usage);

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

        let mut outcome = TurnOutcome {
            result: None,
            usage: tally.usage,
            stop: None,
            unseen,
        };
        match result {
            Some(Ok(result)) => {
                commit(session, &result, &seen).await;
                outcome.stop = turn_stop(limit, &result);
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
        if let Err(problem) = record_prompt(session, text.clone()).await {
            warn!(%problem, "Failed to record a mid-turn notice");
        }
    }
    record_response(session, &result.total_text).await;
    session.save_checkpoint().await;
}
