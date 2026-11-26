//! Running a hosted agent's queued turns.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aura_agent::{AgentLoopEvent, AgentLoopResult};
use aura_core_types::time::now_ms;
use aura_core_types::{AgentId, SubagentExit, SubagentResult, Transaction};
use aura_model::Message;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::agents::inbox::MessageKind;
use crate::agents::mail::{MailMessage, ThreadId};
use crate::agents::observer::HostedTurn;
use crate::scheduler::ScheduleOverrides;

use super::outcome::{
    cancelled, ended, failed, observe_end, observe_start, reply_text, RELAY_DRAIN,
};
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
        let (answer, handles) = TurnAnswer::split(envelope, name);
        let hosted = HostedTurn {
            agent_id,
            parent,
            name: &answer.name,
            model: &model,
            prompt: &prompt,
        };
        let input = TurnInput {
            prompt: &prompt,
            history,
            setup: &setup,
            workspace,
            events: handles.events,
            cancel: handles.cancel,
            stop: &stop,
        };
        let (result, messages) = self.observed_execute(&hosted, input).await;
        let replier = self.record_turn_end(agent_id, &result, messages);
        match handles.waiter {
            Some(waiter) => self.answer_waiter(agent_id, answer, waiter, result),
            None => self.route_answer(agent_id, answer, replier, &result),
        }
        if !self.retire_if_done(agent_id) {
            self.pump(agent_id);
        }
    }

    /// [`Self::execute`] the turn, telling the turn observer (if any)
    /// when it starts and ends and relaying its events.
    async fn observed_execute(
        &self,
        hosted: &HostedTurn<'_>,
        mut input: TurnInput<'_>,
    ) -> (SubagentResult, Option<Vec<Message>>) {
        let agent_id = hosted.agent_id;
        let observer = self.observer.lock().clone();
        let (events, relay) = observe_start(observer.as_ref(), hosted, input.events.take());
        input.events = events;
        let ran = self.execute(agent_id, input).await;
        if let Some(relay) = relay {
            let _ = tokio::time::timeout(RELAY_DRAIN, relay).await;
        }
        observe_end(observer.as_ref(), agent_id, ran)
    }

    /// Mark the turn finished on the agent's entry and keep its history.
    /// Returns the agent's parent and parent thread, or `None` when the
    /// agent was stopped while the turn ran: the entry is gone and nobody
    /// but a waiter should hear about it.
    fn record_turn_end(
        &self,
        agent_id: AgentId,
        result: &SubagentResult,
        messages: Option<Vec<Message>>,
    ) -> Option<Replier> {
        let mut agents = self.agents.lock();
        agents.get_mut(&agent_id).map(|entry| {
            entry.running = false;
            entry.turns += 1;
            if !result.final_message.trim().is_empty() {
                entry.last_reply = Some(result.final_message.clone());
            }
            if let (Some(messages), Body::Hosted { history, .. }) = (messages, &mut entry.body) {
                *history = messages;
            }
            Replier {
                parent: entry.parent(),
                parent_thread: entry.parent_thread.clone(),
            }
        })
    }

    /// Hand a waiting `task` its result. That result never goes through
    /// `enqueue`, so it is posted as mail here, before the parent hears it.
    fn answer_waiter(
        &self,
        agent_id: AgentId,
        answer: TurnAnswer,
        waiter: oneshot::Sender<SubagentResult>,
        result: SubagentResult,
    ) {
        if let Some(parent) = answer.from {
            let to_name = self
                .agents
                .lock()
                .get(&parent)
                .map_or_else(|| "user".to_string(), |e| e.name.clone());
            self.post_mail(MailMessage {
                id: 0,
                thread: answer.thread,
                subject: answer.subject,
                from: agent_id,
                from_name: answer.name,
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

    /// Send the turn's answer as a reply to wherever [`reply_route`] says.
    fn route_answer(
        self: &Arc<Self>,
        agent_id: AgentId,
        answer: TurnAnswer,
        replier: Option<Replier>,
        result: &SubagentResult,
    ) {
        let TurnAnswer {
            from,
            kind,
            thread,
            subject,
            ..
        } = answer;
        let route = replier.and_then(|replier| {
            reply_route(
                kind,
                from,
                replier.parent,
                replier.parent_thread,
                thread,
                subject,
            )
        });
        if let Some((to, choice)) = route {
            let text = reply_text(result);
            let envelope = self.envelope(Some(agent_id), to, MessageKind::Reply, text, choice);
            if let Err(err) = self.enqueue(to, envelope) {
                warn!(agent = %agent_id, %err, "could not deliver reply");
            }
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
    async fn execute(
        &self,
        agent_id: AgentId,
        input: TurnInput<'_>,
    ) -> Result<AgentLoopResult, SubagentResult> {
        let setup = input.setup;
        if setup.timeout == Some(Duration::ZERO) {
            return Err(ended(agent_id, SubagentExit::Timeout));
        }
        let tx = Transaction::user_prompt(agent_id, input.prompt.to_string());
        if let Err(err) = self.store.enqueue_tx(&tx) {
            return Err(failed(agent_id, format!("enqueue prompt: {err}")));
        }
        let schedule = self.scheduler.schedule_agent_with_options(
            agent_id,
            ScheduleOverrides {
                caller_owns_deadline: true,
                loop_config: Some(setup.loop_config.clone()),
                policy: Some(setup.policy.clone()),
                event_tx: input.events,
                workspace_override: input.workspace,
                router_override: Some(setup.router.clone()),
                tools_override: Some(setup.tools.clone()),
                history: input.history,
            },
        );
        let bounded = async {
            match setup.timeout {
                Some(limit) => tokio::time::timeout(limit, schedule).await.ok(),
                None => Some(schedule.await),
            }
        };
        let cancel = input.cancel.unwrap_or_default();
        let processed = tokio::select! {
            biased;
            () = input.stop.cancelled() => return Err(cancelled(agent_id)),
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

/// What [`HostInner::execute`] needs to run one turn.
struct TurnInput<'a> {
    prompt: &'a str,
    history: Vec<Message>,
    setup: &'a RunSetup,
    workspace: Option<(PathBuf, bool)>,
    events: Option<mpsc::Sender<AgentLoopEvent>>,
    cancel: Option<CancellationToken>,
    stop: &'a CancellationToken,
}

/// The answering agent's parent and the thread of the parent's last
/// request, read when its turn ends.
struct Replier {
    parent: Option<AgentId>,
    parent_thread: Option<(ThreadId, String)>,
}

/// Where a turn's message came from, which decides where its answer goes.
struct TurnAnswer {
    from: Option<AgentId>,
    kind: MessageKind,
    thread: ThreadId,
    subject: String,
    /// The answering agent's display name.
    name: String,
}

/// The parts of a turn's envelope that steer the turn itself.
struct TurnHandles {
    waiter: Option<oneshot::Sender<SubagentResult>>,
    events: Option<mpsc::Sender<AgentLoopEvent>>,
    cancel: Option<CancellationToken>,
}

impl TurnAnswer {
    /// Split a turn's `envelope` into what its answer needs (with the
    /// answering agent's `name`), and the turn's [`TurnHandles`].
    fn split(envelope: Envelope, name: String) -> (Self, TurnHandles) {
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
        let answer = Self {
            from,
            kind,
            thread,
            subject,
            name,
        };
        let handles = TurnHandles {
            waiter,
            events,
            cancel,
        };
        (answer, handles)
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
