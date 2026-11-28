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
//! The right column comes back with its agent as well: the Record panel's
//! records and the Sandbox tree are held in the view, highlight, scroll
//! and opened folders included, and the PROCESSES, TRACE and TASKS lists
//! put their highlight back on the row it was on once they are filtered
//! for the agent again. What the session sends on the switch only brings
//! them up to date.
//!
//! A chat that holds its agent's history (`loaded`) comes back on screen
//! at once. One that doesn't waits for it blank until the history (a
//! `LoadConversation`) or word that the live chat is current
//! (`ChatReady`) arrives, and is shown then. A wait longer than
//! [`LOADING_DELAY`] says it is loading, and the chat that ends it fades
//! in over [`FADE_IN`](super::run_log::FADE_IN). The loading state is
//! swapped with the rest of the chat, so an answer for an agent no longer
//! on screen still ends its wait.

use super::chat_view::EarlyPolicy;
use super::run_log::{fade_progress, LOADING_DELAY};
use super::sandbox::SandboxTree;
use super::scroll::ChatPlace;
use super::side_scope::ListPlaces;
use super::{hud::Hud, App, AppState, ChatTab, NotificationType, PendingApproval, SidePanelTab};
use crate::{
    components::Message,
    events::{QueuedItemView, RecordSummary, TraceNode, UiCommand},
    renderer::{format_elapsed, ChatLineCache},
    PermissionMode,
};
use std::collections::{HashMap, VecDeque};
use std::mem;
use std::time::Instant;
mod shed;

/// The chat of an agent that is not on screen.
pub(crate) struct AgentView {
    messages: VecDeque<Message>,
    streaming_content: String,
    early_policies: Vec<EarlyPolicy>,
    status: String,
    processing_since: Option<Instant>,
    scroll: ChatPlace,
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
    /// The Record panel's records, highlight and scroll
    records: VecDeque<RecordSummary>,
    selected_record: usize,
    records_scroll: usize,
    /// The Sandbox tree, with its opened folders and highlight, and scroll
    sandbox: SandboxTree,
    sandbox_scroll: usize,
    /// Where the side panel's lists were, once the agent was on screen
    list_places: Option<ListPlaces>,
    /// Whether the chat was let go of off screen (see `shed`)
    shed: bool,
}

impl Default for AgentView {
    fn default() -> Self {
        Self {
            messages: VecDeque::new(),
            streaming_content: String::new(),
            early_policies: Vec::new(),
            status: "Ready".to_string(),
            processing_since: None,
            scroll: ChatPlace::default(),
            group_open: HashMap::new(),
            chat_lines: ChatLineCache::default(),
            pending_approvals: VecDeque::new(),
            hud: Hud::default(),
            chat_tab: ChatTab::Chat,
            busy: false,
            loaded: false,
            loading_since: None,
            revealed_at: None,
            records: VecDeque::new(),
            selected_record: 0,
            records_scroll: 0,
            sandbox: SandboxTree::default(),
            sandbox_scroll: 0,
            list_places: None,
            shed: false,
        }
    }
}

/// Whether `state` means the chat's agent has a turn in flight.
const fn is_busy(state: AppState) -> bool {
    matches!(state, AppState::Processing | AppState::AwaitingApproval)
}

/// Whether the input line holds an answer to a prompt (an email, a
/// password, a sign-in code) instead of the chat's draft.
const fn input_is_prompt_answer(state: AppState) -> bool {
    matches!(
        state,
        AppState::LoginEmail
            | AppState::LoginPassword
            | AppState::LinkCode
            | AppState::IntegrationInput
    )
}

impl App {
    /// The messages of `agent_id`'s chat: the one on screen, else the one
    /// held for it; `None` when none is held.
    #[must_use]
    pub(crate) fn chat_messages_of(&self, agent_id: &str) -> Option<&VecDeque<Message>> {
        if agent_id == self.active_agent_id {
            return Some(&self.messages);
        }
        self.switching
            .views
            .get(agent_id)
            .map(|view| &view.messages)
    }

    /// Exchange the chat on screen with `view`.
    fn swap_chat(&mut self, view: &mut AgentView) {
        mem::swap(&mut self.messages, &mut view.messages);
        mem::swap(
            &mut self.chat.streaming_content,
            &mut view.streaming_content,
        );
        mem::swap(&mut self.chat.early_policies, &mut view.early_policies);
        mem::swap(&mut self.status, &mut view.status);
        mem::swap(&mut self.ticks.processing_since, &mut view.processing_since);
        view.scroll = self.replace_chat_place(view.scroll);
        mem::swap(&mut self.chat.group_open, &mut view.group_open);
        mem::swap(&mut self.chat.chat_lines, &mut view.chat_lines);
        mem::swap(&mut self.pending.approvals, &mut view.pending_approvals);
        mem::swap(&mut self.hud, &mut view.hud);
        mem::swap(&mut self.switching.chat_loaded, &mut view.loaded);
        mem::swap(
            &mut self.switching.chat_loading_since,
            &mut view.loading_since,
        );
        mem::swap(&mut self.switching.chat_revealed_at, &mut view.revealed_at);
        mem::swap(&mut self.records.items, &mut view.records);
        mem::swap(&mut self.records.selected, &mut view.selected_record);
        mem::swap(&mut self.records.scroll, &mut view.records_scroll);
        mem::swap(&mut self.sandbox, &mut view.sandbox);
        mem::swap(&mut self.side.sandbox_scroll, &mut view.sandbox_scroll);
    }

    /// Have the chat on screen wait for its history when it doesn't hold
    /// it: nothing of it is drawn until the history arrives, so the panel
    /// never shows a partial or out-of-date chat that then jumps.
    pub(super) fn start_chat_loading(&mut self) {
        if !self.switching.chat_loaded {
            self.switching.chat_loading_since = Some(Instant::now());
            self.switching.chat_revealed_at = None;
        }
    }

    /// The chat on screen holds its agent's history now. One that waited
    /// long enough to say it was loading fades in; one that waited less,
    /// or was shown all along, is shown as it is: fading in after a wait
    /// too short to see only holds it back.
    pub(super) fn reveal_chat(&mut self) {
        self.switching.chat_loaded = true;
        if self
            .switching
            .chat_loading_since
            .take()
            .is_some_and(|at| at.elapsed() >= LOADING_DELAY)
        {
            self.switching.chat_revealed_at = Some(Instant::now());
        }
    }

    /// Whether the chat on screen waits for its agent's history.
    #[must_use]
    pub const fn chat_loading(&self) -> bool {
        self.switching.chat_loading_since.is_some()
    }

    /// Whether the chat has waited long enough to say it is loading, so a
    /// quick load shows no message at all.
    #[must_use]
    pub fn chat_loading_shown(&self) -> bool {
        self.switching
            .chat_loading_since
            .is_some_and(|at| at.elapsed() >= LOADING_DELAY)
    }

    /// How far the chat on screen has faded in since its history arrived,
    /// as milliseconds of [`super::run_log::FADE_IN`], while it still is.
    #[must_use]
    pub fn chat_fading(&self) -> Option<(usize, usize)> {
        fade_progress(self.switching.chat_revealed_at)
    }

    /// Whether the chat on screen changes from frame to frame: it waits
    /// for its history, fades in or shows a machine booting, on the CHAT
    /// tab.
    #[must_use]
    pub fn chat_animating(&self) -> bool {
        self.chat_tab() == ChatTab::Chat
            && (self.chat_loading()
                || self.chat_fading().is_some()
                || self.chat_jumping()
                || self.messages.iter().rev().any(Message::is_live_boot))
    }

    /// Whether the agent with hex id `agent_id` has a turn in flight.
    #[must_use]
    pub fn agent_busy(&self, agent_id: &str) -> bool {
        if agent_id == self.active_agent_id {
            is_busy(self.chat_state)
        } else {
            self.switching
                .views
                .get(agent_id)
                .is_some_and(|view| view.busy)
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
        let prompt = input_is_prompt_answer(self.chat_state);
        if !prompt {
            self.swap_draft(&previous, agent_id);
        }
        let outgoing_partial = self.switching.reload_after_turn.take() == Some(previous.clone());
        let mut view = self.switching.views.remove(agent_id).unwrap_or_default();
        let busy = view.busy;
        let incoming_partial = view.shed && busy;
        let tab = mem::replace(&mut view.chat_tab, self.chat_tab());
        self.switching.list_places_due = view.list_places.replace(self.list_places());
        self.put_channel_chat_place_away();
        self.swap_chat(&mut view);
        view.busy = is_busy(self.chat_state);
        view.shed = outgoing_partial;
        self.note_left_screen(&previous);
        self.switching.views.insert(previous, view);
        self.switching.reload_after_turn = incoming_partial.then(|| agent_id.to_string());
        self.shed_views(None);
        if !prompt {
            self.chat_state = self.chat_state_for(busy);
        }
        self.close_detail();
        self.clear_selection();
        Some(tab)
    }

    /// Leave `agent_id` on chat tab `tab`: at once while it is on screen,
    /// else for when it comes back.
    pub(super) fn give_chat_tab_back(&mut self, agent_id: &str, tab: ChatTab) {
        if agent_id == self.active_agent_id {
            self.restore_chat_tab(tab);
        } else if let Some(view) = self.switching.views.get_mut(agent_id) {
            view.chat_tab = tab;
        }
    }

    /// Put the input's draft aside for `previous` and bring back the one
    /// `agent_id` was left with, an empty input for an agent with none.
    /// Runs after a queue edit gave the input back its draft, so that
    /// draft is the one kept. A draft put aside while a prompt held the
    /// input (see [`input_is_prompt_answer`]) was never taken back, and
    /// still waits for its agent's next return.
    fn swap_draft(&mut self, previous: &str, agent_id: &str) {
        let incoming = self.switching.drafts.remove(agent_id).unwrap_or_default();
        let outgoing = self.take_draft(String::new());
        self.restore_draft(incoming);
        if !outgoing.is_empty() {
            self.switching.drafts.insert(previous.to_string(), outgoing);
        }
        // Menus and history browsing followed the old text.
        self.input.reset_slash_menu();
        self.close_mentions();
        self.input.history.reset();
        self.input.command_history.reset();
    }

    /// State of the chat swapped in, whose agent is `busy` or not. A turn
    /// waiting for an approval shows its prompt.
    fn chat_state_for(&self, busy: bool) -> AppState {
        if !self.pending.approvals.is_empty() {
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
                    self.ticks.processing_since
                } else {
                    self.switching
                        .views
                        .get(agent_id)
                        .and_then(|v| v.processing_since)
                };
                let Some(elapsed) = since.map(|since| since.elapsed()) else {
                    return;
                };
                if elapsed < self.ticks.alert_settings.notify_after {
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
        self.ticks.alerts.push(message);
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
            self.agent_graph_upsert(agent_id, &run_id, nodes);
            return;
        }
        if agent_id == self.active_agent_id || self.active_agent_id.is_empty() {
            self.process_command(command);
            return;
        }
        if let UiCommand::SetQueue(queue) = command {
            self.store_offscreen_queue(agent_id, queue);
            return;
        }
        let asks = matches!(command, UiCommand::RequestApproval { .. });
        if shows_active_agent_only(&command) {
            // The Record and Sandbox panels, the plan strip and the Plans
            // and Tasks tabs show the active agent only.
            return;
        }
        self.process_offscreen_command(agent_id, command, asks);
    }

    /// A graph update of any agent: into the trace feed and the graph.
    fn agent_graph_upsert(&mut self, agent_id: &str, run_id: &str, nodes: Vec<TraceNode>) {
        self.trace_feed.push(agent_id, run_id, &nodes);
        let graphed = self.upsert_graph(run_id, nodes);
        // Any agent's step may be in the log on screen, as a subagent's.
        self.log.mark_stale();
        let feed_shown = self.sandbox_panel_visible() && self.side.tab == SidePanelTab::Trace;
        self.ticks.redraw |= graphed || feed_shown;
    }

    /// The queue of an agent off screen.
    fn store_offscreen_queue(&mut self, agent_id: &str, queue: Vec<QueuedItemView>) {
        // The queue panel's state is the agent on screen's; this one
        // only stores the queue, and confirms its edits.
        self.confirm_queued_renames(agent_id, &queue);
        self.switching
            .views
            .entry(agent_id.to_string())
            .or_default()
            .hud
            .queue = queue;
        // A confirmed rename shows in the chat's queued previews.
        self.ticks.redraw = true;
    }

    /// Run `command` on the chat of `agent_id`, off screen, swapped in for
    /// the time being, then tell what came of it.
    fn process_offscreen_command(&mut self, agent_id: &str, command: UiCommand, asks: bool) {
        let mut view = self.switching.views.remove(agent_id).unwrap_or_default();
        let state = self.chat_state;
        let notification = self.notifications.current.take();
        let redraw = self.ticks.redraw;
        let was_busy = view.busy;
        self.swap_chat(&mut view);
        self.chat_state = self.chat_state_for(was_busy);

        self.process_command(command);

        let busy = is_busy(self.chat_state);
        if busy {
            self.ticks.processing_since.get_or_insert_with(Instant::now);
        } else {
            self.ticks.processing_since = None;
        }
        let note = self.notifications.current.take();
        self.chat_state = state;
        self.swap_chat(&mut view);
        view.busy = busy;
        self.switching.views.insert(agent_id.to_string(), view);
        if !busy {
            self.shed_views(Some(agent_id));
        }
        self.ticks.redraw = redraw;

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
                self.notifications.current = Some(noticed);
                self.ticks.redraw = true;
            }
            None => self.notifications.current = notification,
        }
    }

    /// Display name of the agent with hex id `agent_id`.
    fn agent_name(&self, agent_id: &str) -> String {
        self.agents
            .iter()
            .find(|a| a.id == agent_id)
            .map_or_else(|| "Agent".to_string(), |a| a.name.clone())
    }

    /// Note in the Agents panel and in the channels' MEMBERS whether the
    /// agent with hex id `agent_id` is in plan mode, until the next list
    /// says otherwise.
    fn set_agent_plan_mode(&mut self, agent_id: &str, on: bool) {
        if let Some(agent) = self.agents.iter_mut().find(|a| a.id == agent_id) {
            if agent.plan_mode != on {
                agent.plan_mode = on;
                self.ticks.redraw = true;
            }
        }
        if self.set_member_plan_mode(agent_id, on) {
            self.ticks.redraw = true;
        }
    }
}

/// Whether `command` is for panels that show the active agent only.
fn shows_active_agent_only(command: &UiCommand) -> bool {
    matches!(
        command,
        UiCommand::NewRecord(_)
            | UiCommand::ClearRecords
            | UiCommand::SetRecords(_)
            | UiCommand::SetSandboxRoot(_)
            | UiCommand::SetSandboxRemote(_)
            | UiCommand::SandboxListing { .. }
            | UiCommand::SandboxFile { .. }
            | UiCommand::SetPlan(_)
            | UiCommand::SetPlans(_)
            | UiCommand::SetTasks(_)
            | UiCommand::UpsertTask(_)
            | UiCommand::UpsertPlan(_)
    )
}
