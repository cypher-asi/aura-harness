//! The agents panel's CHANNELS tab: the list of channels, and the channel
//! that is open in it, with its coordinator and the agents it spawned.
//!
//! The list's rows are the channels, then "+ New channel" (index
//! `channels.len()`), which the panel pins to its bottom row. An open
//! channel's selectable rows are its back row (0), then one per member
//! (`1 + index`); the title, description and `MEMBERS` heading between
//! them take no highlight.
//!
//! Opening and closing a channel is told to the session (`OpenChannel`,
//! `CloseChannel`) so it can send the channel's posts and tasks. A
//! channel that drops out of `SetChannels` closes here without telling
//! it: the session already knows it is gone.
//!
//! While a channel is open the right column is one tall side panel whose
//! PROJECTS tab, first and selected on opening, shows the channel (see
//! [`App::side_maximized`]). The user's own layout flags are left alone,
//! and the side tab shown before the channel opened comes back when it
//! closes, so closing gives back exactly the layout there was.

use super::processes::SIDE_PAGE;
use super::tasks::TaskList;
use super::{contains, follow_selection, AgentsTab, App, NotificationType, SidePanelTab};
use crate::events::{ChannelPostView, ChannelSummary, TaskView, UiEvent};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use std::collections::HashMap;

/// Rows a PageUp or PageDown moves the highlight by.
const PAGE_STEP: usize = 5;

/// What the CHANNELS tab knows about channels, and where it is in them.
#[derive(Debug, Default)]
pub(crate) struct ChannelState {
    /// Every channel that is not archived, in list order
    list: Vec<ChannelSummary>,
    /// Id of the channel open in the panel
    open: Option<String>,
    /// Tasks of each channel's agents, newest first, by channel id
    tasks: HashMap<String, Vec<TaskView>>,
    /// Posts of each channel, by `seq`, by channel id
    posts: HashMap<String, Vec<ChannelPostView>>,
    /// Highlighted row of the list; `list.len()` is "+ New channel"
    selected: usize,
    /// First visible row of the list
    scroll: usize,
    /// Highlighted row of the open channel: 0 is the back row, then its
    /// members
    selected_row: usize,
    /// First visible member row of the open channel
    members_scroll: usize,
    /// Where the list's rows were drawn this frame
    rows_area: Option<Rect>,
    /// Where "+ New channel" was drawn this frame
    new_channel_area: Option<Rect>,
    /// Where the open channel's back row was drawn this frame
    back_area: Option<Rect>,
    /// Where the open channel's member rows were drawn this frame
    member_rows_area: Option<Rect>,
    /// The open channel's tasks as the side panel's PROJECTS tab lists
    /// them, in the TASKS tab's tree
    task_list: TaskList,
    /// First visible row of `task_list`
    tasks_scroll: usize,
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
    pub(super) fn coordinated_by(&self, agent_id: &str) -> Option<&ChannelSummary> {
        self.open_channel()
            .filter(|channel| channel.coordinator_id == agent_id)
    }

    /// Posts of the open channel, oldest first, when `agent_id` is its
    /// coordinator: its chat shows them. Empty otherwise.
    pub(super) fn coordinator_posts(&self, agent_id: &str) -> &[ChannelPostView] {
        self.coordinated_by(agent_id)
            .and_then(|channel| self.posts.get(&channel.id))
            .map_or(&[], Vec::as_slice)
    }

    /// Rows the highlight can be on in the view that is showing.
    fn selectable_rows(&self) -> usize {
        match self.open_channel() {
            Some(channel) => 1 + channel.members.len(),
            None => self.list.len() + 1,
        }
    }

    fn highlighted(&self) -> usize {
        if self.open.is_some() {
            self.selected_row
        } else {
            self.selected
        }
    }

    fn set_highlighted(&mut self, row: usize) {
        let row = row.min(self.selectable_rows().saturating_sub(1));
        if self.open.is_some() {
            self.selected_row = row;
        } else {
            self.selected = row;
        }
    }

    /// Where the views were drawn is only good for the frame that drew
    /// them; switching views drops it so a click cannot hit the old one.
    fn forget_areas(&mut self) {
        self.rows_area = None;
        self.new_channel_area = None;
        self.back_area = None;
        self.member_rows_area = None;
    }
}

impl App {
    /// Every channel that is not archived, in list order.
    #[must_use]
    pub(crate) fn channels(&self) -> &[ChannelSummary] {
        &self.channels.list
    }

    /// The channel open in the CHANNELS tab, if any.
    #[must_use]
    pub(crate) fn open_channel(&self) -> Option<&ChannelSummary> {
        self.channels.open_channel()
    }

    /// The open channel, while the chat shows its coordinator.
    #[must_use]
    pub(crate) fn coordinated_channel(&self) -> Option<&ChannelSummary> {
        self.channels.coordinated_by(&self.active_agent_id)
    }

    /// The chat panel's title while it shows the open channel's
    /// coordinator, `#slug · coordinator`; `None` for every other agent.
    #[must_use]
    pub(crate) fn coordinator_chat_title(&self) -> Option<String> {
        self.coordinated_channel()
            .map(|channel| format!("#{} · coordinator", channel.slug))
    }

    /// Whether a channel is open in the CHANNELS tab. Only a listed
    /// channel is opened, and one that drops out of the list closes, so
    /// this agrees with [`Self::open_channel`].
    #[must_use]
    pub(crate) const fn channel_open(&self) -> bool {
        self.channels.open.is_some()
    }

    /// Tasks of channel `id`'s agents, newest first.
    #[must_use]
    pub(crate) fn channel_tasks(&self, id: &str) -> &[TaskView] {
        self.channels.tasks.get(id).map_or(&[], Vec::as_slice)
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

    /// Open the listed channel `id` in the CHANNELS tab and tell the
    /// session. A channel that is not listed is not opened.
    pub(crate) fn open_channel_by_id(&mut self, id: &str) {
        if self.channels.find(id).is_none() {
            return;
        }
        if self.channels.open.is_none() {
            self.channels.side_tab_before = Some(self.side_tab);
        }
        self.channels.open = Some(id.to_string());
        self.channels.selected_row = 0;
        self.channels.members_scroll = 0;
        self.channels.forget_areas();
        self.channels.task_list = TaskList::default();
        self.channels
            .task_list
            .set_tasks(self.channel_tasks(id).to_vec());
        self.channels.tasks_scroll = 0;
        self.show_side_tab(SidePanelTab::Projects);
        self.show_agents_tab(AgentsTab::Channels);
        self.try_send_event(UiEvent::OpenChannel(id.to_string()));
    }

    /// Close the open channel, back to the list, and tell the session.
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
        self.channels.task_list = TaskList::default();
        self.channels.tasks_scroll = 0;
        let before = self.channels.side_tab_before.take().unwrap_or_default();
        self.show_side_tab(before);
    }

    /// Say why `/kernel` and `/files` leave the right column as it is:
    /// the open channel's panel fills it until the channel closes, and
    /// the layout they would change comes back then.
    pub(super) fn notify_channel_owns_column(&mut self) {
        self.notification = Some((
            "The channel fills the right column; close it to change the panels".to_string(),
            NotificationType::Warning,
        ));
    }

    /// The open channel's tasks, as the side panel's PROJECTS tab lists
    /// them.
    #[must_use]
    pub(crate) const fn channel_task_list(&self) -> &TaskList {
        &self.channels.task_list
    }

    /// Keep the highlighted channel task inside `visible` rows, returning
    /// the first row to draw.
    pub(crate) fn sync_channel_tasks_scroll(&mut self, visible: usize) -> usize {
        let state = &mut self.channels;
        state.tasks_scroll = follow_selection(
            state.tasks_scroll,
            state.task_list.selected(),
            visible,
            state.task_list.rows().len(),
        );
        state.tasks_scroll
    }

    /// Move the PROJECTS tab's task highlight `delta` rows, as the wheel
    /// does.
    pub(super) fn move_channel_task(&mut self, delta: isize) {
        self.channels.task_list.move_by(delta);
    }

    /// Highlight the PROJECTS tab's task row `index` (clamped).
    pub(super) fn select_channel_task(&mut self, index: usize) {
        self.channels.task_list.select(index);
    }

    /// Fold or open what is under the highlighted channel task row.
    pub(super) fn toggle_channel_task(&mut self) {
        self.channels.task_list.toggle_selected();
    }

    /// Keys of the side panel's PROJECTS tab, which move through the
    /// channel's tasks as the TASKS tab does: Enter shows the agent of
    /// the highlighted task, Right unfolds and Left folds.
    pub(super) fn handle_channel_side_key(&mut self, key: KeyEvent) {
        let list = &mut self.channels.task_list;
        match key.code {
            KeyCode::Up => list.move_by(-1),
            KeyCode::Down => list.move_by(1),
            KeyCode::PageUp => list.move_by(-SIDE_PAGE),
            KeyCode::PageDown => list.move_by(SIDE_PAGE),
            KeyCode::Home => list.select(0),
            KeyCode::End => list.select(usize::MAX),
            KeyCode::Right => list.set_selected_open(true),
            KeyCode::Left => list.set_selected_open(false),
            KeyCode::Enter => self.open_channel_task_agent(),
            _ => {}
        }
    }

    /// Show the agent doing the highlighted channel task in the chat.
    fn open_channel_task_agent(&mut self) {
        let Some(task) = self.channels.task_list.selected_task() else {
            return;
        };
        if task.agent_id.is_empty() || task.agent_id == self.active_agent_id {
            return;
        }
        let target = task.agent_id.clone();
        self.request_agent_switch(target);
    }

    /// Start creating a channel: the "+ New channel" row.
    pub(crate) fn start_new_channel(&mut self) {
        self.notification = Some((
            "New channel form coming".to_string(),
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

    /// Highlighted row of the open channel: 0 is the back row, `1 + i`
    /// is member `i`.
    #[must_use]
    pub(crate) const fn selected_channel_row(&self) -> usize {
        self.channels.selected_row
    }

    /// First visible member row of the open channel.
    #[must_use]
    pub(crate) const fn channel_members_scroll(&self) -> usize {
        self.channels.members_scroll
    }

    /// Keep the list's highlight inside rows `visible_rows` tall.
    /// "+ New channel" is pinned below them, so it never scrolls.
    pub(crate) fn sync_channels_scroll(&mut self, visible_rows: usize) {
        let state = &mut self.channels;
        state.scroll =
            follow_selection(state.scroll, state.selected, visible_rows, state.list.len());
    }

    /// Keep the highlighted member inside rows `visible_rows` tall. The
    /// back row sits above them, so highlighting it leaves them as they
    /// are.
    pub(crate) fn sync_channel_members_scroll(&mut self, visible_rows: usize) {
        let members = self
            .open_channel()
            .map_or(0, |channel| channel.members.len());
        let state = &mut self.channels;
        let Some(member) = state.selected_row.checked_sub(1) else {
            state.members_scroll = state
                .members_scroll
                .min(members.saturating_sub(visible_rows.max(1)));
            return;
        };
        state.members_scroll =
            follow_selection(state.members_scroll, member, visible_rows, members);
    }

    /// Record where the list's rows were drawn this frame.
    pub(crate) fn set_channel_rows_area(&mut self, area: Rect) {
        self.channels.rows_area = Some(area);
    }

    /// Record where "+ New channel" was drawn this frame.
    pub(crate) fn set_new_channel_area(&mut self, area: Rect) {
        self.channels.new_channel_area = Some(area);
    }

    /// Record where the open channel's back row was drawn this frame.
    pub(crate) fn set_channel_back_area(&mut self, area: Rect) {
        self.channels.back_area = Some(area);
    }

    /// Record where the open channel's member rows were drawn this frame.
    pub(crate) fn set_channel_member_rows_area(&mut self, area: Rect) {
        self.channels.member_rows_area = Some(area);
    }

    pub(super) fn set_channels(&mut self, list: Vec<ChannelSummary>) {
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
        self.channels.selected = self.channels.selected.min(self.channels.list.len());
        let rows = self.channels.selectable_rows();
        if self.channels.open.is_some() {
            self.channels.selected_row = self.channels.selected_row.min(rows.saturating_sub(1));
        }
    }

    pub(super) fn set_channel_tasks(&mut self, channel_id: String, tasks: Vec<TaskView>) {
        if self.channels.open.as_deref() == Some(channel_id.as_str()) {
            // The tree keeps its own copy, to fold and highlight in.
            self.channels.task_list.set_tasks(tasks.clone());
        }
        self.channels.tasks.insert(channel_id, tasks);
    }

    pub(super) fn set_channel_posts(&mut self, channel_id: String, posts: Vec<ChannelPostView>) {
        self.channels.posts.insert(channel_id, posts);
    }

    /// Add `post` to its channel, or replace the post with its `seq`.
    pub(super) fn add_channel_post(&mut self, channel_id: String, post: ChannelPostView) {
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

    /// Selectable row drawn at a screen cell, in the view that is showing.
    pub(super) fn channel_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let state = &self.channels;
        match state.open_channel() {
            Some(channel) => {
                if contains(state.back_area, column, row) {
                    return Some(0);
                }
                let rows = state
                    .member_rows_area
                    .filter(|&rows| contains(Some(rows), column, row))?;
                let member = state.members_scroll + usize::from(row - rows.y);
                (member < channel.members.len()).then_some(1 + member)
            }
            None => {
                if contains(state.new_channel_area, column, row) {
                    return Some(state.list.len());
                }
                let rows = state
                    .rows_area
                    .filter(|&rows| contains(Some(rows), column, row))?;
                let index = state.scroll + usize::from(row - rows.y);
                (index < state.list.len()).then_some(index)
            }
        }
    }

    /// Clicking a row highlights it and acts on it, as Enter does.
    pub(super) fn click_channel_row(&mut self, column: u16, row: u16) {
        let Some(index) = self.channel_row_at(column, row) else {
            return;
        };
        self.channels.set_highlighted(index);
        self.activate_channel_row();
    }

    /// Move the highlight `delta` rows, as the wheel does.
    pub(super) fn move_channel_highlight(&mut self, delta: isize) {
        let row = self.channels.highlighted().saturating_add_signed(delta);
        self.channels.set_highlighted(row);
    }

    /// Keys of the CHANNELS tab. Returns whether the key was used.
    pub(super) fn handle_channels_key(&mut self, key: KeyEvent) -> bool {
        let current = self.channels.highlighted();
        match key.code {
            KeyCode::Up => self.channels.set_highlighted(current.saturating_sub(1)),
            KeyCode::Down => self.channels.set_highlighted(current + 1),
            KeyCode::PageUp => self
                .channels
                .set_highlighted(current.saturating_sub(PAGE_STEP)),
            KeyCode::PageDown => self.channels.set_highlighted(current + PAGE_STEP),
            KeyCode::Home => self.channels.set_highlighted(0),
            KeyCode::End => self.channels.set_highlighted(usize::MAX),
            KeyCode::Enter => self.activate_channel_row(),
            KeyCode::Esc | KeyCode::Backspace if self.channel_open() => self.close_channel(),
            KeyCode::Left | KeyCode::Right => self.shift_agents_tab(key.code),
            _ => return false,
        }
        true
    }

    /// Act on the highlighted row: open a channel, start a new one, go
    /// back to the list, or show a member's chat.
    fn activate_channel_row(&mut self) {
        let Some(channel) = self.channels.open_channel() else {
            let selected = self.channels.list.get(self.channels.selected);
            match selected.map(|channel| channel.id.clone()) {
                Some(id) => self.open_channel_by_id(&id),
                None => self.start_new_channel(),
            }
            return;
        };
        let Some(member) = self.channels.selected_row.checked_sub(1) else {
            self.close_channel();
            return;
        };
        if let Some(id) = channel.members.get(member).map(|agent| agent.id.clone()) {
            self.request_agent_switch(id);
        }
    }
}
