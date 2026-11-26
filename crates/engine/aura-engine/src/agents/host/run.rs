//! Running a hosted agent's queued turns.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aura_agent::{AgentLoopEvent, AgentLoopResult};
use aura_core_types::time::now_ms;
use aura_core_types::{AgentId, SubagentExit, SubagentResult, Transaction};
use aura_model::Message;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::agents::inbox::MessageKind;
use crate::agents::mail::{MailMessage, ThreadId};
use crate::agents::observer::{HostedTurn, TurnObserver};
use crate::scheduler::ScheduleOverrides;

use super::{Body, Envelope, HostInner, RunSetup, ThreadChoice};

impl HostInner {
    /// Start the next queued turn if the agent is free to run one.
    pub(super) fn pump(self: &Arc<Self>, agent_id: AgentId) {
        let job = {
            let mut agents = self.agents.lock();
            let Some(entry) = agents.get_mut(&agent_id) else {
                return;
            };
            if entry.running || entry.paused {
                return;
            }
            let parent = entry.parent();
            let Body::Hosted {
                setup,
                history,
                queue,
            } = &mut entry.body
            else {
                return;
            };
            let Some(envelope) = queue.pop_front() else {
                return;
            };
            if envelope.kind == MessageKind::Request
                && envelope.from.is_some_and(|from| Some(from) == parent)
            {
                entry.parent_thread = Some((envelope.thread.clone(), envelope.subject.clone()));
            }
            entry.running = true;
            let prompt = render_for_hosted(&envelope, parent);
            Turn {
                envelope,
                prompt,
                history: history.clone(),
                setup: setup.clone(),
                workspace: entry.env.workspace.clone(),
                stop: entry.stop.clone(),
                name: entry.name.clone(),
                model: entry.model.clone(),
                parent,
            }
        };
        let host = self.clone();
        tokio::spawn(async move { host.run_turn(agent_id, job).await });
    }

    async fn run_turn(self: Arc<Self>, agent_id: AgentId, turn: Turn) {
        let Turn {
            envelope,
            prompt,
            history,
            setup,
            workspace,
            stop,
            name,
            model,
            parent,
        } = turn;
        let Envelope {
            from,
            kind,
            thread,
            subject,
            waiter,
            events,
            cancel,
            ..
        } = envelope;
        let observer = self.observer.lock().clone();
        let (events, relay) = match &observer {
            Some(observer) => {
                observer.turn_started(&HostedTurn {
                    agent_id,
                    parent,
                    name: &name,
                    model: &model,
                    prompt: &prompt,
                });
                let (tx, relay) = relay_events(Arc::clone(observer), agent_id, events);
                (Some(tx), Some(relay))
            }
            None => (events, None),
        };
        let ran = self
            .execute(
                agent_id, &prompt, history, &setup, workspace, events, cancel, &stop,
            )
            .await;
        if let Some(relay) = relay {
            let _ = tokio::time::timeout(RELAY_DRAIN, relay).await;
        }
        let (result, messages) = match ran {
            Ok(loop_result) => {
                let exit = exit_of(&loop_result);
                if let Some(observer) = &observer {
                    observer.turn_ended(agent_id, Some(&loop_result), &exit);
                }
                finished(agent_id, loop_result, exit)
            }
            Err(early) => {
                if let Some(observer) = &observer {
                    observer.turn_ended(agent_id, None, &early.exit);
                }
                (early, None)
            }
        };

        // Stopped while the turn ran: the entry is gone and nobody but a
        // waiter should hear about it.
        let replier = {
            let mut agents = self.agents.lock();
            agents.get_mut(&agent_id).map(|entry| {
                entry.running = false;
                entry.turns += 1;
                if !result.final_message.trim().is_empty() {
                    entry.last_reply = Some(result.final_message.clone());
                }
                if let (Some(messages), Body::Hosted { history, .. }) = (messages, &mut entry.body)
                {
                    *history = messages;
                }
                (entry.parent(), entry.parent_thread.clone())
            })
        };

        match waiter {
            Some(waiter) => {
                // A waiting `task` result never goes through `enqueue`, so
                // it is posted as mail here, before the parent hears it.
                if let Some(parent) = from {
                    let to_name = self
                        .agents
                        .lock()
                        .get(&parent)
                        .map_or_else(|| "user".to_string(), |e| e.name.clone());
                    self.post_mail(MailMessage {
                        id: 0,
                        thread,
                        subject,
                        from: agent_id,
                        from_name: name,
                        to: parent,
                        to_name,
                        kind: MessageKind::Reply,
                        text: reply_text(&result),
                        sent_ms: now_ms(),
                        subject_from_text: false,
                    });
                }
                let _ = waiter.send(result);
            }
            None => {
                let route = replier.and_then(|(parent, parent_thread)| {
                    reply_route(kind, from, parent, parent_thread, thread, subject)
                });
                if let Some((to, choice)) = route {
                    let text = reply_text(&result);
                    let envelope =
                        self.envelope(Some(agent_id), to, MessageKind::Reply, text, choice);
                    if let Err(err) = self.enqueue(to, envelope) {
                        warn!(agent = %agent_id, %err, "could not deliver reply");
                    }
                }
            }
        }
        if !self.retire_if_done(agent_id) {
            self.pump(agent_id);
        }
    }

    /// Remove a `task` child, and everything it started, once it has
    /// nothing left to do. Returns whether it was removed.
    fn retire_if_done(&self, agent_id: AgentId) -> bool {
        let dropped = {
            let mut agents = self.agents.lock();
            let done = agents.get(&agent_id).is_some_and(|entry| {
                entry.one_shot
                    && !entry.running
                    && matches!(&entry.body, Body::Hosted { queue, .. } if queue.is_empty())
            });
            if !done {
                return false;
            }
            self.remove_subtree(&mut agents, agent_id, true)
        };
        self.notify_dropped(dropped);
        true
    }

    /// Run one turn of a hosted agent through the scheduler. `Err` is how
    /// a turn that produced no loop result ended.
    #[allow(clippy::too_many_arguments)]
    async fn execute(
        &self,
        agent_id: AgentId,
        prompt: &str,
        history: Vec<Message>,
        setup: &RunSetup,
        workspace: Option<(PathBuf, bool)>,
        events: Option<mpsc::Sender<AgentLoopEvent>>,
        cancel: Option<CancellationToken>,
        stop: &CancellationToken,
    ) -> Result<AgentLoopResult, SubagentResult> {
        if setup.timeout == Some(Duration::ZERO) {
            return Err(ended(agent_id, SubagentExit::Timeout));
        }
        let tx = Transaction::user_prompt(agent_id, prompt.to_string());
        if let Err(err) = self.store.enqueue_tx(&tx) {
            return Err(failed(agent_id, format!("enqueue prompt: {err}")));
        }
        let schedule = self.scheduler.schedule_agent_with_options(
            agent_id,
            ScheduleOverrides {
                caller_owns_deadline: true,
                loop_config: Some(setup.loop_config.clone()),
                policy: Some(setup.policy.clone()),
                event_tx: events,
                workspace_override: workspace,
                router_override: Some(setup.router.clone()),
                tools_override: Some(setup.tools.clone()),
                history,
            },
        );
        let bounded = async {
            match setup.timeout {
                Some(limit) => tokio::time::timeout(limit, schedule).await.ok(),
                None => Some(schedule.await),
            }
        };
        let cancel = cancel.unwrap_or_default();
        let processed = tokio::select! {
            biased;
            () = stop.cancelled() => return Err(cancelled(agent_id)),
            () = cancel.cancelled() => return Err(cancelled(agent_id)),
            outcome = bounded => match outcome {
                None => return Err(ended(agent_id, SubagentExit::Timeout)),
                Some(Err(err)) => return Err(failed(agent_id, format!("schedule: {err}"))),
                Some(Ok(processed)) => processed,
            },
        };
        processed
            .last_result
            .ok_or_else(|| failed(agent_id, "the turn produced no result".into()))
    }
}

struct Turn {
    envelope: Envelope,
    prompt: String,
    history: Vec<Message>,
    setup: Arc<RunSetup>,
    workspace: Option<(PathBuf, bool)>,
    stop: CancellationToken,
    name: String,
    model: String,
    parent: Option<AgentId>,
}

/// Where a hosted agent's answer to a turn goes, and in which thread.
///
/// The answer to a request goes back to its sender in the request's
/// thread. The answer to a reply from a child goes up to the agent's
/// parent, in the thread of the parent's last request
/// (`parent_thread`), or the pair's latest thread when there was none.
fn reply_route(
    kind: MessageKind,
    from: Option<AgentId>,
    parent: Option<AgentId>,
    parent_thread: Option<(ThreadId, String)>,
    thread: ThreadId,
    subject: String,
) -> Option<(AgentId, ThreadChoice<'static>)> {
    match kind {
        MessageKind::Request => from.map(|from| (from, ThreadChoice::Existing(thread, subject))),
        MessageKind::Reply => parent.map(|parent| {
            let choice = parent_thread.map_or(ThreadChoice::Resolve(None), |(thread, subject)| {
                ThreadChoice::Existing(thread, subject)
            });
            (parent, choice)
        }),
    }
}

/// The prompt a hosted agent sees for `envelope`. Its parent talks to it
/// directly; anyone else is introduced, with the thread's subject.
fn render_for_hosted(envelope: &Envelope, parent: Option<AgentId>) -> String {
    let Some(from) = envelope.from else {
        return envelope.text.clone();
    };
    match envelope.kind {
        MessageKind::Request if Some(from) == parent => envelope.text.clone(),
        MessageKind::Request => format!(
            "Message from agent {} ({from}):\nSubject: {}\n\n{}\n\nYour final answer is sent \
             back to {}.",
            envelope.from_name, envelope.subject, envelope.text, envelope.from_name
        ),
        MessageKind::Reply => format!(
            "Agent {} ({from}) reported back:\nSubject: {}\n\n{}",
            envelope.from_name, envelope.subject, envelope.text
        ),
    }
}

pub(super) fn reply_text(result: &SubagentResult) -> String {
    match &result.exit {
        SubagentExit::Completed if result.final_message.trim().is_empty() => {
            "(finished without a reply)".to_string()
        }
        SubagentExit::Completed => result.final_message.clone(),
        SubagentExit::Failed { reason } => format!("failed: {reason}"),
        SubagentExit::Cancelled => "was cancelled".to_string(),
        SubagentExit::Timeout => "timed out".to_string(),
        SubagentExit::Rejected { reason } => format!("rejected: {reason}"),
    }
}

/// How a turn that produced `result` ended.
fn exit_of(result: &AgentLoopResult) -> SubagentExit {
    if result.output_truncated || result.insufficient_credits || result.stalled {
        SubagentExit::Failed {
            reason: "child execution stopped before completion".into(),
        }
    } else if result.timed_out {
        SubagentExit::Timeout
    } else {
        result
            .llm_error
            .as_ref()
            .map_or(SubagentExit::Completed, |reason| SubagentExit::Failed {
                reason: reason.clone(),
            })
    }
}

/// The outcome of a turn that produced `result`, and the history it
/// leaves.
fn finished(
    agent_id: AgentId,
    result: AgentLoopResult,
    exit: SubagentExit,
) -> (SubagentResult, Option<Vec<Message>>) {
    let outcome = SubagentResult {
        child_agent_id: Some(agent_id),
        final_message: result.final_text,
        total_input_tokens: result.total_input_tokens,
        total_output_tokens: result.total_output_tokens,
        files_changed: result
            .file_changes
            .into_iter()
            .map(|change| change.path)
            .collect(),
        exit,
    };
    (outcome, Some(result.messages))
}

/// How long the end of a turn waits for its events to reach the
/// observer.
const RELAY_DRAIN: Duration = Duration::from_secs(5);

/// A sender for a turn's events that shows each to `observer`, in order,
/// then passes it on to `downstream`. The task ends once every sender is
/// gone.
fn relay_events(
    observer: Arc<dyn TurnObserver>,
    agent_id: AgentId,
    downstream: Option<mpsc::Sender<AgentLoopEvent>>,
) -> (mpsc::Sender<AgentLoopEvent>, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<AgentLoopEvent>(1024);
    let relay = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            observer.on_event(agent_id, &event);
            if let Some(downstream) = &downstream {
                let _ = downstream.send(event).await;
            }
        }
    });
    (tx, relay)
}

fn ended(agent_id: AgentId, exit: SubagentExit) -> SubagentResult {
    SubagentResult {
        child_agent_id: Some(agent_id),
        final_message: String::new(),
        total_input_tokens: 0,
        total_output_tokens: 0,
        files_changed: Vec::new(),
        exit,
    }
}

pub(super) fn cancelled(agent_id: AgentId) -> SubagentResult {
    ended(agent_id, SubagentExit::Cancelled)
}

pub(super) fn failed(agent_id: AgentId, reason: String) -> SubagentResult {
    ended(agent_id, SubagentExit::Failed { reason })
}
