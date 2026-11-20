//! One chat per agent.
//!
//! Agents work in parallel, so the chat on screen is only one of several
//! that are changing. The chat of the active agent lives in the `App`
//! fields the rest of the UI reads; the chats of the other agents are kept
//! as [`AgentView`]s and swapped in when their agent becomes active.

use super::{hud::Hud, App, AppState, NotificationType, PendingApproval};
use crate::{
    components::Message,
    events::UiCommand,
    renderer::{format_elapsed, ChatLineCache},
};
use std::collections::{HashMap, VecDeque};
use std::mem;
use std::time::Instant;

/// The chat of an agent that is not on screen.
pub(crate) struct AgentView {
    messages: VecDeque<Message>,
    streaming_content: String,
    status: String,
    processing_since: Option<Instant>,
    scroll_offset: usize,
    group_open: HashMap<u64, bool>,
    chat_lines: ChatLineCache,
    pending_approvals: VecDeque<PendingApproval>,
    hud: Hud,
    /// Whether the agent has a turn in flight
    busy: bool,
}

impl Default for AgentView {
    fn default() -> Self {
        Self {
            messages: VecDeque::new(),
            streaming_content: String::new(),
            status: "Ready".to_string(),
            processing_since: None,
            scroll_offset: 0,
            group_open: HashMap::new(),
            chat_lines: ChatLineCache::default(),
            pending_approvals: VecDeque::new(),
            hud: Hud::default(),
            busy: false,
        }
    }
}

/// Whether `state` means the chat's agent has a turn in flight.
const fn is_busy(state: AppState) -> bool {
    matches!(state, AppState::Processing | AppState::AwaitingApproval)
}

/// Whether `state` is a prompt or overlay rather than the chat itself.
const fn is_modal(state: AppState) -> bool {
    !matches!(
        state,
        AppState::Idle | AppState::Processing | AppState::AwaitingApproval
    )
}

impl App {
    /// Exchange the chat on screen with `view`.
    fn swap_chat(&mut self, view: &mut AgentView) {
        mem::swap(&mut self.messages, &mut view.messages);
        mem::swap(&mut self.streaming_content, &mut view.streaming_content);
        mem::swap(&mut self.status, &mut view.status);
        mem::swap(&mut self.processing_since, &mut view.processing_since);
        mem::swap(&mut self.scroll_offset, &mut view.scroll_offset);
        mem::swap(&mut self.group_open, &mut view.group_open);
        mem::swap(&mut self.chat_lines, &mut view.chat_lines);
        mem::swap(&mut self.pending_approvals, &mut view.pending_approvals);
        mem::swap(&mut self.hud, &mut view.hud);
    }

    /// Whether the agent with hex id `agent_id` has a turn in flight.
    #[must_use]
    pub fn agent_busy(&self, agent_id: &str) -> bool {
        if agent_id == self.active_agent_id {
            is_busy(self.state)
        } else {
            self.views.get(agent_id).is_some_and(|view| view.busy)
        }
    }

    /// Show the chat of `agent_id`, keeping the one on screen for when its
    /// agent is shown again.
    pub(super) fn show_agent_chat(&mut self, agent_id: &str) {
        let previous = mem::replace(&mut self.active_agent_id, agent_id.to_string());
        if previous == agent_id || previous.is_empty() {
            return;
        }
        self.leave_queue_panel();
        let mut view = self.views.remove(agent_id).unwrap_or_default();
        let busy = view.busy;
        self.swap_chat(&mut view);
        view.busy = is_busy(self.state);
        self.views.insert(previous, view);
        if !is_modal(self.state) {
            self.state = self.chat_state(busy);
        }
        self.close_detail();
        self.clear_selection();
    }

    /// State of the chat swapped in, whose agent is `busy` or not. A turn
    /// waiting for an approval shows its prompt.
    fn chat_state(&self, busy: bool) -> AppState {
        if !self.pending_approvals.is_empty() {
            AppState::AwaitingApproval
        } else if busy {
            AppState::Processing
        } else {
            AppState::Idle
        }
    }

    /// Note something about `agent_id` that the user should notice even
    /// when looking elsewhere: it needs an approval, or a long turn ended.
    fn queue_alert(&mut self, agent_id: &str, command: &UiCommand) {
        let on_screen = agent_id == self.active_agent_id || self.active_agent_id.is_empty();
        let message = match command {
            UiCommand::RequestApproval { tool, .. } => {
                format!("{} needs approval to run {tool}", self.agent_name(agent_id))
            }
            UiCommand::Complete => {
                let since = if on_screen {
                    self.processing_since
                } else {
                    self.views.get(agent_id).and_then(|v| v.processing_since)
                };
                let Some(elapsed) = since.map(|since| since.elapsed()) else {
                    return;
                };
                if elapsed < self.alert_settings.notify_after {
                    return;
                }
                format!(
                    "{} finished after {}",
                    self.agent_name(agent_id),
                    format_elapsed(elapsed)
                )
            }
            _ => return,
        };
        self.alerts.push(message);
    }

    /// Apply `command` to the chat of `agent_id`, on screen or not.
    pub(super) fn process_agent_command(&mut self, agent_id: &str, command: UiCommand) {
        self.queue_alert(agent_id, &command);
        if let UiCommand::GraphUpsert { run_id, nodes } = command {
            self.trace_feed.push(agent_id, &run_id, &nodes);
            self.upsert_graph(&run_id, nodes);
            return;
        }
        if agent_id == self.active_agent_id || self.active_agent_id.is_empty() {
            self.process_command(command);
            return;
        }
        if let UiCommand::SetQueue(queue) = command {
            // The queue panel's state is the agent on screen's; this one
            // only stores the queue, and confirms its edits.
            self.confirm_queued_renames(agent_id, &queue);
            self.views
                .entry(agent_id.to_string())
                .or_default()
                .hud
                .queue = queue;
            return;
        }
        let asks = matches!(command, UiCommand::RequestApproval { .. });
        if matches!(
            command,
            UiCommand::NewRecord(_)
                | UiCommand::ClearRecords
                | UiCommand::SetRecords(_)
                | UiCommand::SetSandboxRoot(_)
                | UiCommand::SetPlan(_)
                | UiCommand::SetPlans(_)
                | UiCommand::SetTasks(_)
                | UiCommand::UpsertTask(_)
        ) {
            // The Record and Sandbox panels, the plan strip and the Plans
            // and Tasks tabs show the active agent only.
            return;
        }

        let mut view = self.views.remove(agent_id).unwrap_or_default();
        let state = self.state;
        let notification = self.notification.take();
        let was_busy = view.busy;
        self.swap_chat(&mut view);
        self.state = self.chat_state(was_busy);

        self.process_command(command);

        let busy = is_busy(self.state);
        if busy {
            self.processing_since.get_or_insert_with(Instant::now);
        } else {
            self.processing_since = None;
        }
        let note = self.notification.take();
        self.state = state;
        self.swap_chat(&mut view);
        view.busy = busy;
        self.views.insert(agent_id.to_string(), view);

        let name = self.agent_name(agent_id);
        self.notification = match note {
            Some((message, kind)) => Some((format!("{name}: {message}"), kind)),
            None if asks => Some((
                format!("{name} needs your approval; switch to it to answer"),
                NotificationType::Warning,
            )),
            None if was_busy && !busy => {
                Some((format!("{name} finished"), NotificationType::Success))
            }
            None => notification,
        };
    }

    /// Display name of the agent with hex id `agent_id`.
    fn agent_name(&self, agent_id: &str) -> String {
        self.agents
            .iter()
            .find(|a| a.id == agent_id)
            .map_or_else(|| "Agent".to_string(), |a| a.name.clone())
    }
}
