//! One chat per agent.
//!
//! Agents work in parallel, so the chat on screen is only one of several
//! that are changing. The chat of the active agent lives in the `App`
//! fields the rest of the UI reads; the chats of the other agents are kept
//! as [`AgentView`]s and swapped in when their agent becomes active. Each
//! view also keeps the chat panel's tab its agent was left on, so the
//! agent comes back on it; an agent never shown opens on CHAT. What was
//! typed on the input line stays with its agent too: leaving an agent puts
//! its draft aside, and coming back to it puts the draft back.
//!
//! A chat that holds its agent's history (`loaded`) comes back on screen
//! at once. One that doesn't waits for it blank, says it is loading after
//! [`LOADING_DELAY`], and fades in over [`FADE_IN`](super::run_log::FADE_IN)
//! once the history (a
//! `LoadConversation`) or word that the live chat is current
//! (`ChatReady`) arrives. The loading state is swapped with the rest of
//! the chat, so an answer for an agent no longer on screen still ends its
//! wait.

use super::run_log::{fade_progress, LOADING_DELAY};
use super::{hud::Hud, App, AppState, ChatTab, NotificationType, PendingApproval, SidePanelTab};
use crate::{
    components::Message,
    events::UiCommand,
    renderer::{format_elapsed, ChatLineCache},
    PermissionMode,
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
    /// The chat panel's tab the agent was left on, shown again with it
    chat_tab: ChatTab,
    /// Whether the agent has a turn in flight
    busy: bool,
    /// Whether the chat holds the agent's history, so showing it again
    /// needs no reload. A chat built only from what the agent sent while
    /// off screen does not.
    pub(super) loaded: bool,
    /// When the chat started waiting for its history
    loading_since: Option<Instant>,
    /// When the history it waited for arrived, to fade it in
    revealed_at: Option<Instant>,
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
            chat_tab: ChatTab::Chat,
            busy: false,
            loaded: false,
            loading_since: None,
            revealed_at: None,
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

/// Whether the input line holds an answer to a prompt (an email, a
/// password, a sign-in code) instead of the chat's draft.
const fn input_is_prompt_answer(state: AppState) -> bool {
    matches!(
        state,
        AppState::LoginEmail | AppState::LoginPassword | AppState::LinkCode
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
        mem::swap(&mut self.chat_loaded, &mut view.loaded);
        mem::swap(&mut self.chat_loading_since, &mut view.loading_since);
        mem::swap(&mut self.chat_revealed_at, &mut view.revealed_at);
    }

    /// Have the chat on screen wait for its history when it doesn't hold
    /// it: nothing of it is drawn until the history arrives, so the panel
    /// never shows a partial or out-of-date chat that then jumps.
    pub(super) fn start_chat_loading(&mut self) {
        if !self.chat_loaded {
            self.chat_loading_since = Some(Instant::now());
            self.chat_revealed_at = None;
        }
    }

    /// The chat on screen holds its agent's history now. One that was
    /// waiting for it fades in; one shown all along stays as it is.
    pub(super) fn reveal_chat(&mut self) {
        self.chat_loaded = true;
        if self.chat_loading_since.take().is_some() {
            self.chat_revealed_at = Some(Instant::now());
        }
    }

    /// Whether the chat on screen waits for its agent's history.
    #[must_use]
    pub const fn chat_loading(&self) -> bool {
        self.chat_loading_since.is_some()
    }

    /// Whether the chat has waited long enough to say it is loading, so a
    /// quick load shows no message at all.
    #[must_use]
    pub fn chat_loading_shown(&self) -> bool {
        self.chat_loading_since
            .is_some_and(|at| at.elapsed() >= LOADING_DELAY)
    }

    /// How far the chat on screen has faded in since its history arrived,
    /// as milliseconds of [`super::run_log::FADE_IN`], while it still is.
    #[must_use]
    pub fn chat_fading(&self) -> Option<(usize, usize)> {
        fade_progress(self.chat_revealed_at)
    }

    /// Whether the chat on screen changes from frame to frame: it waits
    /// for its history or fades in, on the CHAT tab.
    #[must_use]
    pub fn chat_animating(&self) -> bool {
        self.chat_tab() == ChatTab::Chat && (self.chat_loading() || self.chat_fading().is_some())
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

    /// Show the chat of `agent_id`, keeping the one on screen, and the tab
    /// the chat panel is on, for when its agent is shown again. Returns the
    /// tab the agent coming on screen was left on, CHAT for one never
    /// shown, or `None` when no chat was swapped: the agent is already on
    /// screen, or none was before it.
    pub(super) fn show_agent_chat(&mut self, agent_id: &str) -> Option<ChatTab> {
        let previous = mem::replace(&mut self.active_agent_id, agent_id.to_string());
        if previous == agent_id || previous.is_empty() {
            return None;
        }
        self.leave_queue_panel();
        if !input_is_prompt_answer(self.state) {
            self.swap_draft(&previous, agent_id);
        }
        let mut view = self.views.remove(agent_id).unwrap_or_default();
        let busy = view.busy;
        let tab = mem::replace(&mut view.chat_tab, self.chat_tab());
        self.swap_chat(&mut view);
        view.busy = is_busy(self.state);
        self.views.insert(previous, view);
        if !is_modal(self.state) {
            self.state = self.chat_state(busy);
        }
        self.close_detail();
        self.clear_selection();
        Some(tab)
    }

    /// Put the input's draft aside for `previous` and bring back the one
    /// `agent_id` was left with, an empty input for an agent with none.
    /// Runs after a queue edit gave the input back its draft, so that
    /// draft is the one kept. A draft put aside while a prompt held the
    /// input (see [`input_is_prompt_answer`]) was never taken back, and
    /// still waits for its agent's next return.
    fn swap_draft(&mut self, previous: &str, agent_id: &str) {
        let incoming = self.drafts.remove(agent_id).unwrap_or_default();
        let outgoing = self.take_draft(String::new());
        self.restore_draft(incoming);
        if !outgoing.is_empty() {
            self.drafts.insert(previous.to_string(), outgoing);
        }
        // Menus and history browsing followed the old text.
        self.reset_slash_menu();
        self.close_mentions();
        self.input_history.reset();
        self.command_history.reset();
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

    /// Apply `command` to the chat of `agent_id`, on screen or not. One
    /// for a chat off screen asks for a frame ([`Self::take_redraw`]) only
    /// when it changes something on screen.
    pub(super) fn process_agent_command(&mut self, agent_id: &str, command: UiCommand) {
        self.queue_alert(agent_id, &command);
        if let UiCommand::SetMode(mode) = &command {
            // The Agents panel marks every agent in plan mode, on screen
            // or not; the chat's mode line follows below as before.
            self.set_agent_plan_mode(agent_id, *mode == PermissionMode::Plan);
        }
        if let UiCommand::GraphUpsert { run_id, nodes } = command {
            self.trace_feed.push(agent_id, &run_id, &nodes);
            let graphed = self.upsert_graph(&run_id, nodes);
            // Any agent's step may be in the log on screen, as a subagent's.
            self.log.mark_stale();
            let feed_shown = (self.sandbox_panel_visible || self.channel_open())
                && self.side_tab == SidePanelTab::Trace;
            self.redraw |= graphed || feed_shown;
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
            // A confirmed rename shows in the chat's queued previews.
            self.redraw = true;
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
        let redraw = self.redraw;
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
        self.redraw = redraw;

        let name = self.agent_name(agent_id);
        let noticed = match note {
            Some((message, kind)) => Some((format!("{name}: {message}"), kind)),
            None if asks => Some((
                format!("{name} needs your approval; switch to it to answer"),
                NotificationType::Warning,
            )),
            None if was_busy && !busy => {
                Some((format!("{name} finished"), NotificationType::Success))
            }
            None => None,
        };
        match noticed {
            Some(noticed) => {
                self.notification = Some(noticed);
                self.redraw = true;
            }
            None => self.notification = notification,
        }
    }

    /// Display name of the agent with hex id `agent_id`.
    fn agent_name(&self, agent_id: &str) -> String {
        self.agents
            .iter()
            .find(|a| a.id == agent_id)
            .map_or_else(|| "Agent".to_string(), |a| a.name.clone())
    }

    /// Note in the Agents panel whether the agent with hex id `agent_id`
    /// is in plan mode, until the next list says otherwise.
    fn set_agent_plan_mode(&mut self, agent_id: &str, on: bool) {
        if let Some(agent) = self.agents.iter_mut().find(|a| a.id == agent_id) {
            if agent.plan_mode != on {
                agent.plan_mode = on;
                self.redraw = true;
            }
        }
    }
}
