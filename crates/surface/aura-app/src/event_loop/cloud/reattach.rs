//! Following again, after a restart, a cloud turn the app quit during:
//! the machine ran it on with no one attached ([`CloudHub::reattach_turn`]).
//! The turn ends through the loop as one started here does
//! ([`super::handle_ended`]), so what it was asked is answered from the
//! conversation copied back.
//!
//! # Failure modes
//!
//! A turn whose run is gone, or that can't be followed, is judged by the
//! conversation its machine left: one that ends on the agent's reply
//! finished, and is answered. Any other was cut short: it is not given
//! up but asked to carry on, its reports still owed.

use super::forward::{forward, TurnEnd, TurnExtras, WaitingMark};
use super::submit::{copied_session, STOPPED};
use super::*;
use crate::event_loop::notices::Notice;
use aura_agent::checkpoint::Checkpoints;
use aura_model::{ContentBlock, Message, Role};
use tokio::sync::mpsc;

/// What the chat says as the turn is followed again.
const REATTACHED: &str = "The app restarted while this agent worked in the cloud; following its \
turn again. What it did meanwhile is shown again below.";

/// The PROCESSES row's label of a reattached turn.
const REATTACHED_ROW: &str = "Turn followed again after a restart";

/// What the chat says as an agent cut short carries on.
const CARRIES_ON: &str = "↻ Carrying on after the app restarted";

impl CloudState {
    /// The agents with a cloud turn (or `/new`) under way, with the run
    /// on its node each turn streams from, once the hub knows it.
    pub(in crate::event_loop) fn turns_under_way(&self) -> Vec<(AgentId, Option<String>)> {
        let hub = self.hub();
        let agents: Vec<AgentId> = self.shared.running.lock().keys().copied().collect();
        agents
            .into_iter()
            .map(|agent| (agent, hub.as_ref().and_then(|hub| hub.turn_run(agent))))
            .collect()
    }
}

/// Follow `agent`'s cloud turn in node run `run` again, as a turn of it
/// started here: running, with its row, until the loop hears it ended.
/// When it turns out cut short, `resume` asks it to carry on. Returns
/// whether it is followed: not without the cloud, or once the agent is
/// no longer placed.
pub(in crate::event_loop) async fn reattach(
    state: &LoopState<'_>,
    agent: AgentId,
    run: String,
    resume: String,
) -> bool {
    let Some(hub) = state.cloud.hub() else {
        return false;
    };
    if !state.cloud.is_cloud(agent) {
        return false;
    }
    info!(%agent, %run, "After the restart: following the cloud turn again");
    let ui = AgentUi::new(state.commands.clone(), agent);
    ui.send(UiCommand::ShowMessage(MessageData::new(
        MessageRole::System,
        REATTACHED.to_string(),
    )))
    .await;
    let cancel = mark_running(state, agent).await;
    let follow = Follow::new(state, agent, resume);
    let rx = hub.reattach_turn(agent, run, cancel.clone());
    tokio::spawn(follow.run(ui, rx, cancel));
    true
}

/// [`reattach`], the turn's events coming from `rx` instead of its
/// machine.
#[cfg(test)]
pub(in crate::event_loop) async fn follow_events(
    state: &LoopState<'_>,
    agent: AgentId,
    rx: mpsc::Receiver<RemoteEvent>,
    resume: String,
) {
    let ui = AgentUi::new(state.commands.clone(), agent);
    let cancel = mark_running(state, agent).await;
    tokio::spawn(Follow::new(state, agent, resume).run(ui, rx, cancel));
}

/// Note `agent` as running a cloud turn, as a turn started here is, and
/// give it its PROCESSES row. Returns what stops it.
async fn mark_running(state: &LoopState<'_>, agent: AgentId) -> CancellationToken {
    state.hub.standing.started(agent);
    let cancel = CancellationToken::new();
    state
        .cloud
        .shared
        .running
        .lock()
        .insert(agent, cancel.clone());
    lock(&state.hub.cloud_turns).insert(agent);
    state.hub.status.clear(agent);
    crate::event_loop::agents::send_agent_list(state).await;
    let row = crate::event_loop::handlers::turn_summary(agent, REATTACHED_ROW, "");
    state.processes.start(row, Some(cancel.clone()));
    cancel
}

/// What a followed turn's task needs.
struct Follow {
    agent: AgentId,
    shared: Arc<Shared>,
    processes: Arc<crate::event_loop::processes::ProcessTable>,
    sessions: Option<Checkpoints>,
    notices: crate::event_loop::notices::NoticeSender,
    resume: String,
    extras: TurnExtras,
}

impl Follow {
    fn new(state: &LoopState<'_>, agent: AgentId, resume: String) -> Self {
        Self {
            agent,
            shared: Arc::clone(&state.cloud.shared),
            processes: Arc::clone(&state.processes),
            sessions: state.hub.factory.checkpoints.clone(),
            notices: state.hub.notices.clone(),
            resume,
            extras: TurnExtras {
                waiting: Some(WaitingMark {
                    hub: Arc::clone(&state.hub),
                    agent,
                }),
                ..TurnExtras::default()
            },
        }
    }

    /// Show the turn in its chat until it ends, then tell the loop how it
    /// went, or ask the agent to carry on when it was cut short.
    async fn run(
        mut self,
        ui: AgentUi,
        mut rx: mpsc::Receiver<RemoteEvent>,
        cancel: CancellationToken,
    ) {
        let agent = self.agent;
        let end = forward(&ui, &mut rx, &mut self.extras, &cancel).await;
        let stopped = cancel.is_cancelled();
        self.processes.finish(
            &crate::event_loop::processes::turn_id(agent),
            end.status(stopped),
        );
        let copied = copied_session(&ui, &mut rx, self.sessions.take(), agent).await;
        let messages = copied.map(|checkpoint| checkpoint.messages);
        let finished = messages.as_deref().is_some_and(ends_on_a_reply);
        let failure = match end {
            _ if stopped => Some(STOPPED.to_string()),
            TurnEnd::Done => None,
            TurnEnd::Failed(_) if finished => None,
            TurnEnd::Failed(why) => {
                info!(%agent, %why, "The followed turn was cut short; asking it to carry on");
                self.carry_on();
                self.shared.end_turn(agent, None);
                return;
            }
        };
        self.shared
            .end_turn(agent, Some(TurnOutcome { failure, messages }));
    }

    /// Ask the agent to carry on: it starts once the loop frees it.
    fn carry_on(&self) {
        let notice = Notice {
            to: self.agent,
            shown: CARRIES_ON.to_string(),
            prompt: self.resume.clone(),
            inbox: None,
        };
        if self.notices.send(notice).is_err() {
            warn!(agent = %self.agent, "The terminal is shutting down; the agent is not asked to carry on");
        }
    }
}

/// Whether `messages`, a conversation, ends on the agent's reply, with
/// no tool call left to answer: its turn finished.
fn ends_on_a_reply(messages: &[Message]) -> bool {
    messages.last().is_some_and(|last| {
        last.role == Role::Assistant
            && !last
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
            && !last.text_content().trim().is_empty()
    })
}
