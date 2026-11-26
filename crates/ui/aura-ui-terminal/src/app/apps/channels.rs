//! Channels, an app of their own beside the agents: the agents panel's
//! CHANNELS tab lists them, the chat panel holds the open channel's
//! conversation and nothing else, and the right column is one panel with
//! the channel's MEMBERS and INFO tabs.
//!
//! The agents panel's tab picks the app on screen (see `apps`). A channel
//! is open only while CHANNELS shows:
//!
//! - Leaving CHANNELS closes the open channel (telling the session,
//!   `CloseChannel`); the app the user goes to puts back its own agent.
//! - Coming to CHANNELS opens the channel that was open on leaving, if it
//!   is still listed, else the highlighted or first channel. So does a
//!   list arriving while CHANNELS shows with none open.
//! - The session putting on screen the coordinator of another listed
//!   channel (one just made, say) opens that channel.
//! - The session putting on screen an agent that is not a member of the
//!   open channel (a notification jumping to it, say) is the user going
//!   to that agent: the panel moves to PROJECTS (ARCHIVE for an archived
//!   agent) with it on screen.
//!
//! The list's rows are the channels, then "+ New channel" (index
//! `channels.len()`), which the panel pins to its bottom row. Enter or a
//! click opens a channel, or starts a new one; the list stays on screen.
//!
//! Opening a channel is told to the session (`OpenChannel`), which puts
//! the coordinator on screen and sends the channel's posts. A channel
//! that drops out of `SetChannels` closes here without telling it: the
//! session already knows it is gone.
//!
//! While a channel is open the right column is one tall side panel with
//! only MEMBERS and INFO (see [`App::side_maximized`]). The user's own
//! layout flags are left alone, and the side tab shown before the channel
//! opened comes back when it closes, so leaving gives back exactly the
//! layout there was. With CHANNELS showing and no channel open there is
//! nothing to show beside the list: the right column hides, and the chat
//! asks for a channel to be picked.

use crate::app::processes::SIDE_PAGE;
use crate::app::scroll::InfoScroll;
use crate::app::{
    contains, follow_selection, AgentsTab, App, ChatTab, NotificationType, SidePanelTab,
};
use crate::events::{ChannelPostView, ChannelSummary, UiEvent};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use std::collections::HashMap;

/// Rows a PageUp or PageDown moves the highlight by.
const PAGE_STEP: usize = 5;

/// What the CHANNELS app knows about channels, and where it is in them.
#[derive(Debug, Default)]
pub(crate) struct ChannelState {
    /// Every channel that is not archived, in list order
    list: Vec<ChannelSummary>,
    /// Id of the open channel
    open: Option<String>,
    /// Posts of each channel, by `seq`, by channel id
    posts: HashMap<String, Vec<ChannelPostView>>,
    /// Highlighted row of the list; `list.len()` is "+ New channel"
    selected: usize,
    /// First visible row of the list
    scroll: usize,
    /// Where the list's rows were drawn this frame
    rows_area: Option<Rect>,
    /// Where "+ New channel" was drawn this frame
    new_channel_area: Option<Rect>,
    /// Whether the session has yet to put a member of the open channel
    /// on screen. Opening a channel asks it for the coordinator, and
    /// until it comes the agent before it is still on screen; that agent
    /// is not a member, but it is not the user leaving either.
    awaiting_member: bool,
    /// Highlighted row of the MEMBERS side tab
    member_selected: usize,
    /// First visible row of the MEMBERS side tab
    member_scroll: usize,
    /// Where the MEMBERS side tab's rows were drawn this frame
    member_rows_area: Option<Rect>,
    /// How far the INFO side tab is scrolled
    info_scroll: InfoScroll,
    /// The side tab shown before the channel opened, shown again when it
    /// closes
    side_tab_before: Option<SidePanelTab>,
}

impl ChannelState {
    fn find(&self, id: &str) -> Option<&ChannelSummary> {
        self.list.iter().find(|channel| channel.id == id)
    }

    fn open_channel(&self) -> Option<&ChannelSummary> {
        self.open.as_deref().and_then(|id| self.find(id))
    }

    /// The open channel, when `agent_id` is its coordinator.
    pub(in crate::app) fn coordinated_by(&self, agent_id: &str) -> Option<&ChannelSummary> {
        self.open_channel()
            .filter(|channel| channel.coordinator_id == agent_id)
    }

    /// Posts of the open channel, oldest first, when `agent_id` is its
    /// coordinator: its chat shows them. Empty otherwise.
    pub(in crate::app) fn coordinator_posts(&self, agent_id: &str) -> &[ChannelPostView] {
        self.coordinated_by(agent_id)
            .and_then(|channel| self.posts.get(&channel.id))
            .map_or(&[], Vec::as_slice)
    }

    /// Whether `agent_id` is the open channel's coordinator or one of its
    /// members. `false` with no channel open.
    fn is_open_member(&self, agent_id: &str) -> bool {
        self.open_channel()
            .is_some_and(|channel| is_member(channel, agent_id))
    }

    /// Members of the open channel: none with no channel open.
    fn open_members(&self) -> usize {
        self.open_channel()
            .map_or(0, |channel| channel.members.len())
    }

    fn set_selected(&mut self, row: usize) {
        self.selected = row.min(self.list.len());
    }

    fn set_member_selected(&mut self, row: usize) {
        self.member_selected = row.min(self.open_members().saturating_sub(1));
    }

    /// Where the views were drawn is only good for the frame that drew
    /// them; switching views drops it so a click cannot hit the old one.
    fn forget_areas(&mut self) {
        self.rows_area = None;
        self.new_channel_area = None;
        self.member_rows_area = None;
    }
}

/// Whether `agent_id` is `channel`'s coordinator or one of its members.
fn is_member(channel: &ChannelSummary, agent_id: &str) -> bool {
    channel.coordinator_id == agent_id || channel.members.iter().any(|m| m.id == agent_id)
}

impl App {
    /// Every channel that is not archived, in list order.
    #[must_use]
    pub(crate) fn channels(&self) -> &[ChannelSummary] {
        &self.channels.list
    }

    /// The open channel, if any.
    #[must_use]
    pub(crate) fn open_channel(&self) -> Option<&ChannelSummary> {
        self.channels.open_channel()
    }

    /// Row of the list the open channel is on.
    #[must_use]
    pub(crate) fn open_channel_row(&self) -> Option<usize> {
        let open = self.channels.open.as_deref()?;
        self.channels
            .list
            .iter()
            .position(|channel| channel.id == open)
    }

    /// The open channel, while the chat shows its coordinator.
    #[must_use]
    pub(crate) fn coordinated_channel(&self) -> Option<&ChannelSummary> {
        self.channels.coordinated_by(&self.active_agent_id)
    }

    /// Whether the channel app is on screen: the agents panel shows
    /// CHANNELS, so the chat and the right column are the channels'.
    #[must_use]
    pub(crate) const fn channel_app(&self) -> bool {
        matches!(self.agents_tab, AgentsTab::Channels)
    }

    /// Codes of the members `text`, typed in the coordinator's chat, goes
    /// to instead of the coordinator: `None` when it mentions no member
    /// or addresses the coordinator too (`@coordinator` or its code), so
    /// the coordinator takes it. Matches the app's routing: `@CODE`,
    /// case-insensitive, trailing punctuation ignored.
    #[must_use]
    pub(crate) fn routed_members(&self, text: &str) -> Option<Vec<String>> {
        const TRAILING: &[char] = &[',', '.', ':', ';', '!', '?', ')', '"', '\''];
        let channel = self.coordinated_channel()?;
        let mut codes: Vec<String> = Vec::new();
        for word in text.split_whitespace() {
            let Some(name) = word.strip_prefix('@') else {
                continue;
            };
            let name = name.trim_end_matches(TRAILING);
            if name.is_empty() {
                continue;
            }
            if name.eq_ignore_ascii_case("coordinator") {
                return None;
            }
            let Some(member) = channel.members.iter().find(|member| {
                member
                    .code
                    .as_deref()
                    .is_some_and(|code| code.eq_ignore_ascii_case(name))
            }) else {
                continue;
            };
            if member.id == channel.coordinator_id {
                return None;
            }
            let code = member.code.clone().unwrap_or_default();
            if !codes.contains(&code) {
                codes.push(code);
            }
        }
        (!codes.is_empty()).then_some(codes)
    }

    /// The chat panel's title in the channel app: `#slug`, with the
    /// channel's title as its info. `None` with no channel open.
    #[must_use]
    pub(crate) fn channel_chat_title(&self) -> Option<(String, String)> {
        self.open_channel()
            .map(|channel| (format!("#{}", channel.slug), channel.title.clone()))
    }

    /// What the empty input says in the channel app: who a message goes
    /// to, or that a channel has to be picked first. `None` outside it.
    #[must_use]
    pub(crate) fn channel_placeholder(&self) -> Option<String> {
        if !self.channel_app() {
            return None;
        }
        Some(self.open_channel().map_or_else(
            || "Pick a channel".to_string(),
            |channel| format!("Message #{}", channel.slug),
        ))
    }

    /// Whether a channel is open. Only a listed channel is opened, and
    /// one that drops out of the list closes, so this agrees with
    /// [`Self::open_channel`].
    #[must_use]
    pub(crate) const fn channel_open(&self) -> bool {
        self.channels.open.is_some()
    }

    /// Posts of channel `id`, oldest first.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "the chat reads them through `coordinator_posts`")
    )]
    #[must_use]
    pub(crate) fn channel_posts(&self, id: &str) -> &[ChannelPostView] {
        self.channels.posts.get(id).map_or(&[], Vec::as_slice)
    }

    /// The user left the channel app for another tab, which remembered
    /// the open channel: close it. The agent that was on screen before
    /// gets its chat tab back from `switch_app`.
    pub(in crate::app) fn leave_channel_app(&mut self) {
        self.close_channel();
    }

    /// The channel open when the user last left CHANNELS, if it is still
    /// listed.
    pub(in crate::app) fn channel_to_reopen(&self) -> Option<String> {
        self.apps
            .channel
            .clone()
            .filter(|id| self.channels.find(id).is_some())
    }

    /// The channel CHANNELS opens on coming to it: the one it was left
    /// on, else the highlighted one, else the first.
    pub(in crate::app) fn channel_to_show(&self) -> Option<String> {
        let list = &self.channels.list;
        self.channel_to_reopen()
            .or_else(|| list.get(self.channels.selected).map(|c| c.id.clone()))
            .or_else(|| list.first().map(|c| c.id.clone()))
    }

    /// Put CHAT on the chat panel, with nothing covering it.
    fn show_channel_chat(&mut self) {
        self.close_detail();
        if self.chat_tab() != ChatTab::Chat {
            self.show_chat_tab(ChatTab::Chat);
        }
    }

    /// Open the listed channel `id` in the channel app and tell the
    /// session. A channel that is not listed, or is open already, is
    /// left as it is.
    pub(crate) fn open_channel_by_id(&mut self, id: &str) {
        let Some(row) = self.channels.list.iter().position(|c| c.id == id) else {
            return;
        };
        if self.channels.open.as_deref() == Some(id) {
            return;
        }
        self.switch_app(AgentsTab::Channels);
        if self.channels.open.is_none() {
            self.channels.side_tab_before = Some(self.side_tab);
        }
        self.channels.open = Some(id.to_string());
        if self.apps.channel.as_deref() != Some(id) {
            self.channels.member_selected = 0;
            self.channels.member_scroll = 0;
            self.channels.info_scroll = InfoScroll::default();
        }
        self.apps.channel = Some(id.to_string());
        self.channels.selected = row;
        self.channels.forget_areas();
        self.show_side_tab(SidePanelTab::ChannelMembers);
        self.show_channel_chat();
        // The coordinator comes on screen now, as a listed agent does when
        // clicked, rather than once the session answers: until then the
        // chat would show the agent before it under the channel's title.
        // Its chat shows at once when held, else waits blank for its
        // history and fades in. The request goes out before `OpenChannel`,
        // so the session has made it active by then and does not load it
        // a second time.
        let coordinator = self.channels.list[row].coordinator_id.clone();
        let switched = !coordinator.is_empty() && self.switch_on_screen(&coordinator);
        self.channels.awaiting_member =
            !switched && !self.channels.is_open_member(&self.active_agent_id);
        self.keep_channel_focus_on_screen();
        self.try_send_event(UiEvent::OpenChannel(id.to_string()));
    }

    /// Close the open channel and tell the session.
    pub(crate) fn close_channel(&mut self) {
        if self.channels.open.take().is_none() {
            return;
        }
        self.left_channel();
        self.try_send_event(UiEvent::CloseChannel);
    }

    /// Undo what opening the channel did to the panels, now that
    /// `channels.open` is `None`: show the side tab there was before it.
    fn left_channel(&mut self) {
        self.channels.forget_areas();
        self.channels.awaiting_member = false;
        let before = self.channels.side_tab_before.take().unwrap_or_default();
        self.show_side_tab(before);
    }

    /// The session put the active agent on screen. The coordinator of
    /// another listed channel opens that channel. While a channel is open,
    /// a member coming ends the wait for the coordinator; any other agent,
    /// once that wait is over, means the user went to it, so the agent app
    /// listing it comes to the front with it on screen.
    pub(in crate::app) fn follow_active_agent_out_of_channel(&mut self) {
        let coordinated = self
            .channels
            .list
            .iter()
            .find(|channel| channel.coordinator_id == self.active_agent_id)
            .map(|channel| channel.id.clone());
        if let Some(id) = coordinated {
            if self.channels.open.as_deref() != Some(id.as_str()) {
                self.open_channel_by_id(&id);
                return;
            }
        }
        if !self.channel_open() {
            return;
        }
        if self.channels.is_open_member(&self.active_agent_id) {
            self.channels.awaiting_member = false;
            return;
        }
        if self.channels.awaiting_member {
            return;
        }
        self.follow_into_agent_app();
    }

    /// Say why the agent panels (`/kernel`, `/files`, `/inbox`, ...) are
    /// not shown: the channel app has only its chat and its own panel.
    pub(in crate::app) fn notify_channel_owns_column(&mut self) {
        self.notification = Some((
            "Channels have only a chat; switch to PROJECTS for agent panels".to_string(),
            NotificationType::Warning,
        ));
    }

    /// Highlighted row of the list, where `channels().len()` is
    /// "+ New channel".
    #[must_use]
    pub(crate) const fn selected_channel(&self) -> usize {
        self.channels.selected
    }

    /// First visible row of the list.
    #[must_use]
    pub(crate) const fn channels_scroll(&self) -> usize {
        self.channels.scroll
    }

    /// Keep the list's highlight inside rows `visible_rows` tall.
    /// "+ New channel" is pinned below them, so it never scrolls.
    pub(crate) fn sync_channels_scroll(&mut self, visible_rows: usize) {
        let state = &mut self.channels;
        state.scroll =
            follow_selection(state.scroll, state.selected, visible_rows, state.list.len());
    }

    /// Record where the list's rows were drawn this frame.
    pub(crate) fn set_channel_rows_area(&mut self, area: Rect) {
        self.channels.rows_area = Some(area);
    }

    /// Record where "+ New channel" was drawn this frame.
    pub(crate) fn set_new_channel_area(&mut self, area: Rect) {
        self.channels.new_channel_area = Some(area);
    }

    pub(in crate::app) fn set_channels(&mut self, list: Vec<ChannelSummary>) {
        self.channels.list = list;
        let gone = self
            .channels
            .open
            .as_deref()
            .is_some_and(|id| self.channels.find(id).is_none());
        if gone {
            self.channels.open = None;
            self.left_channel();
        }
        if self.channel_to_reopen().is_none() {
            self.apps.channel = None;
        }
        if let Some(row) = self.open_channel_row() {
            self.channels.selected = row;
        }
        self.channels.selected = self.channels.selected.min(self.channels.list.len());
        let members = self.channels.open_members();
        self.channels.member_selected =
            self.channels.member_selected.min(members.saturating_sub(1));
        if self.channel_app() && !self.channel_open() {
            if let Some(id) = self.channel_to_show() {
                self.open_channel_by_id(&id);
            }
        }
    }

    pub(in crate::app) fn set_channel_posts(
        &mut self,
        channel_id: String,
        posts: Vec<ChannelPostView>,
    ) {
        self.channels.posts.insert(channel_id, posts);
    }

    /// Add `post` to its channel, or replace the post with its `seq`.
    pub(in crate::app) fn add_channel_post(&mut self, channel_id: String, post: ChannelPostView) {
        let posts = self.channels.posts.entry(channel_id).or_default();
        match posts.binary_search_by_key(&post.seq, |known| known.seq) {
            Ok(at) => {
                if let Some(known) = posts.get_mut(at) {
                    *known = post;
                }
            }
            Err(at) => posts.insert(at, post),
        }
    }

    /// Row of the list drawn at a screen cell: a channel, or
    /// "+ New channel" (`channels().len()`).
    pub(in crate::app) fn channel_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let state = &self.channels;
        if contains(state.new_channel_area, column, row) {
            return Some(state.list.len());
        }
        let rows = state
            .rows_area
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = state.scroll + usize::from(row - rows.y);
        (index < state.list.len()).then_some(index)
    }

    /// Clicking a row highlights it and acts on it, as Enter does.
    pub(in crate::app) fn click_channel_row(&mut self, column: u16, row: u16) {
        let Some(index) = self.channel_row_at(column, row) else {
            return;
        };
        self.channels.set_selected(index);
        self.activate_channel_row();
    }

    /// Move the highlight `delta` rows, as the wheel does.
    pub(in crate::app) fn move_channel_highlight(&mut self, delta: isize) {
        let row = self.channels.selected.saturating_add_signed(delta);
        self.channels.set_selected(row);
    }

    /// The channel row of the list a right click at a screen cell is on,
    /// which it highlights. `None` on "+ New channel" or empty space.
    pub(in crate::app) fn channel_menu_target_at(
        &mut self,
        column: u16,
        row: u16,
    ) -> Option<String> {
        let index = self.channel_row_at(column, row)?;
        let id = self.channels.list.get(index)?.id.clone();
        self.channels.set_selected(index);
        Some(id)
    }

    /// The agent on screen when the user left the agents for CHANNELS,
    /// else the one on screen now (empty when none is).
    pub(in crate::app) fn agent_before_channels(&self) -> &str {
        self.apps
            .chat_tab_before
            .as_ref()
            .map_or(self.active_agent_id.as_str(), |(agent, _)| agent.as_str())
    }

    /// Ask to confirm archiving channel `id`, as deleting a permanent
    /// agent asks: the next key is `y` to go ahead, anything else cancels.
    pub(in crate::app) fn request_archive_channel(&mut self, id: String) {
        let Some(slug) = self.channels.find(&id).map(|channel| channel.slug.clone()) else {
            return;
        };
        self.notification = Some((
            format!("Archive channel #{slug}? Press y to confirm"),
            NotificationType::Warning,
        ));
        self.pending_archive_channel = Some(id);
    }

    /// Resolve a pending channel archive with `key`. Returns `false` when
    /// nothing was pending.
    pub(in crate::app) fn handle_pending_archive_channel(&mut self, key: KeyEvent) -> bool {
        let Some(id) = self.pending_archive_channel.take() else {
            return false;
        };
        if matches!(key.code, KeyCode::Char('y' | 'Y')) {
            self.try_send_event(UiEvent::ArchiveChannel(id));
        } else {
            self.notification = Some(("Cancelled".to_string(), NotificationType::Warning));
        }
        true
    }

    /// Keys of the CHANNELS tab. Returns whether the key was used.
    pub(in crate::app) fn handle_channels_key(&mut self, key: KeyEvent) -> bool {
        let current = self.channels.selected;
        match key.code {
            KeyCode::Up => self.channels.set_selected(current.saturating_sub(1)),
            KeyCode::Down => self.channels.set_selected(current + 1),
            KeyCode::PageUp => self
                .channels
                .set_selected(current.saturating_sub(PAGE_STEP)),
            KeyCode::PageDown => self.channels.set_selected(current + PAGE_STEP),
            KeyCode::Home => self.channels.set_selected(0),
            KeyCode::End => self.channels.set_selected(usize::MAX),
            KeyCode::Enter => self.activate_channel_row(),
            KeyCode::Left | KeyCode::Right => self.shift_agents_tab(key.code),
            _ => return false,
        }
        true
    }

    /// Act on the highlighted row: open its channel, or start a new one.
    fn activate_channel_row(&mut self) {
        let selected = self.channels.list.get(self.channels.selected);
        match selected.map(|channel| channel.id.clone()) {
            Some(id) => self.open_channel_by_id(&id),
            None => self.start_new_channel(),
        }
    }

    // ---- The MEMBERS side tab ------------------------------------------

    /// Highlighted row of the MEMBERS side tab.
    #[must_use]
    pub(crate) const fn selected_channel_member(&self) -> usize {
        self.channels.member_selected
    }

    /// Keep the highlighted member inside rows `visible_rows` tall,
    /// returning the first row to draw.
    pub(crate) fn sync_channel_members_scroll(&mut self, visible_rows: usize) -> usize {
        let members = self.channels.open_members();
        let state = &mut self.channels;
        state.member_scroll = follow_selection(
            state.member_scroll,
            state.member_selected,
            visible_rows,
            members,
        );
        state.member_scroll
    }

    /// Record where the MEMBERS side tab's rows were drawn this frame.
    pub(crate) fn set_channel_member_rows_area(&mut self, area: Rect) {
        self.channels.member_rows_area = Some(area);
    }

    /// Drop where the MEMBERS side tab's rows were drawn, for a frame
    /// that may not draw them.
    pub(in crate::app) fn forget_channel_member_rows(&mut self) {
        self.channels.member_rows_area = None;
    }

    /// Member row of the MEMBERS side tab drawn at a screen cell.
    pub(in crate::app) fn channel_member_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let state = &self.channels;
        let rows = state
            .member_rows_area
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = state.member_scroll + usize::from(row - rows.y);
        (index < state.open_members()).then_some(index)
    }

    /// Highlight member row `index` (clamped). Members only show; going
    /// to a member's own chat would leave the channel app.
    pub(in crate::app) fn select_channel_member(&mut self, index: usize) {
        self.channels.set_member_selected(index);
    }

    /// Move the MEMBERS highlight `delta` rows, as the wheel does.
    pub(in crate::app) fn move_channel_member(&mut self, delta: isize) {
        let row = self.channels.member_selected.saturating_add_signed(delta);
        self.channels.set_member_selected(row);
    }

    /// Clicking a member row highlights it.
    pub(in crate::app) fn click_channel_member(&mut self, column: u16, row: u16) {
        if let Some(index) = self.channel_member_row_at(column, row) {
            self.select_channel_member(index);
        }
    }

    /// Keys of the MEMBERS side tab, which move its highlight.
    pub(in crate::app) fn handle_channel_members_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.move_channel_member(-1),
            KeyCode::Down => self.move_channel_member(1),
            KeyCode::PageUp => self.move_channel_member(-SIDE_PAGE),
            KeyCode::PageDown => self.move_channel_member(SIDE_PAGE),
            KeyCode::Home => self.select_channel_member(0),
            KeyCode::End => self.select_channel_member(usize::MAX),
            _ => {}
        }
    }

    // ---- The INFO side tab ---------------------------------------------

    /// Keep the INFO side tab's scroll inside a document `total` lines
    /// long shown `visible` lines at a time, returning the first line to
    /// show.
    pub(crate) fn sync_channel_info_scroll(&mut self, total: usize, visible: usize) -> usize {
        self.channels.info_scroll.sync(total, visible)
    }

    /// Scroll the INFO side tab by `delta` lines, as the wheel does.
    pub(in crate::app) fn scroll_channel_info(&mut self, delta: isize) {
        self.channels.info_scroll.by(delta);
    }

    /// Keys of the INFO side tab, which scroll it.
    pub(in crate::app) fn handle_channel_info_key(&mut self, key: KeyEvent) {
        self.channels.info_scroll.key(key.code);
    }
}
