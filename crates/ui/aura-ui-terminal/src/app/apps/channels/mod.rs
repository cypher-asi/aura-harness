//! Channels, an app of their own beside the agents: the agents panel's
//! CHANNELS tab lists them, the chat panel holds the open channel's
//! conversation and nothing else, and the right column has the channel's
//! TASKS above a panel with its MEMBERS and INFO tabs.
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
//!   agent) with it on screen. A member on screen keeps the channel open
//!   and shows its own chat (see `channel_member_chat`).
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
//! While a channel is open the Record panel's place holds the channel's
//! TASKS: every plan and task in the channel's project, by every agent
//! working there (see [`RecordsTab::ChannelTasks`]). Below it the side
//! panel lists only MEMBERS and INFO. The user's own layout flags are
//! left alone, and the tabs shown before the channel opened come back
//! when it closes, so leaving gives back exactly the layout there was.
//! With CHANNELS showing and no channel open there is nothing to show
//! beside the list: the right column hides, and the chat asks for a
//! channel to be picked.

mod list;
mod side;

pub(crate) use side::CHANNEL_MEMBER_ROWS;

use crate::app::scroll::InfoScroll;
use crate::app::{AgentsTab, App, ChatTab, RecordsTab, SidePanelTab};
use crate::events::{ChannelPostView, ChannelSummary, UiEvent};
use ratatui::layout::Rect;
use std::collections::HashMap;

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
    /// The Kernel panel's tab shown before the channel opened, shown
    /// again when it closes
    records_tab_before: Option<RecordsTab>,
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
    /// channel's title as its info. `None` with no channel open. Over a
    /// member's chat the panel draws [`Self::member_chat_title`] instead,
    /// with its close button.
    #[must_use]
    pub(crate) fn channel_chat_title(&self) -> Option<(String, String)> {
        self.open_channel()
            .map(|channel| (format!("#{}", channel.slug), channel.title.clone()))
    }

    /// What the empty input says in the channel app: who a message goes
    /// to, a member by its code, or that a channel has to be picked first.
    /// `None` outside it.
    #[must_use]
    pub(crate) fn channel_placeholder(&self) -> Option<String> {
        if !self.channel_app() {
            return None;
        }
        if let Some(member) = self.member_chat() {
            let name = member.code.as_deref().unwrap_or(&member.name);
            return Some(format!("Message {name}"));
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
    pub(in crate::app) fn show_channel_chat(&mut self) {
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
            self.channels.side_tab_before = Some(self.side.tab);
            self.channels.records_tab_before = Some(self.records.tab);
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
        self.show_records_tab(RecordsTab::ChannelTasks);
        self.refresh_side_scope();
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
    /// `channels.open` is `None`: show the tabs there were before it, and
    /// TASKS lists the side panel's scope again.
    fn left_channel(&mut self) {
        self.channels.forget_areas();
        self.channels.awaiting_member = false;
        let before = self.channels.side_tab_before.take().unwrap_or_default();
        self.show_side_tab(before);
        // Set rather than shown: the agent the app goes to is not on
        // screen yet, and POLICIES must not ask for the policies of the
        // one leaving.
        self.records.tab = self.channels.records_tab_before.take().unwrap_or_default();
        self.refresh_side_scope();
    }
}
