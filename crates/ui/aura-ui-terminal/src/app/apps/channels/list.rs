//! The CHANNELS list: following the agent on screen, the list and its
//! posts arriving, its rows, archiving a channel, and its keys.

use crate::app::wheel_views::WheelList;
use crate::app::{contains, App, NotificationType};
use crate::events::{ChannelPostView, ChannelSummary, UiEvent};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

/// Rows a PageUp or PageDown moves the highlight by.
const PAGE_STEP: usize = 5;

impl App {
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
        self.notifications.current = Some((
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
        state.scroll = self.panels.wheel.place(
            WheelList::Channels,
            (state.scroll, state.selected),
            (visible_rows, state.list.len()),
        );
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
        let member_chat_was_open = self.member_chat().is_some();
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
        let selected = self.channels.member_selected;
        self.channels.set_member_selected(selected);
        self.leave_dropped_member_chat(member_chat_was_open);
        if self.channel_app() && !self.channel_open() {
            if let Some(id) = self.channel_to_show() {
                self.open_channel_by_id(&id);
            }
        }
    }

    /// Note in every channel's MEMBERS whether the agent with hex id
    /// `agent_id` is in plan mode, until the next list says otherwise.
    /// Returns whether a row changed.
    pub(in crate::app) fn set_member_plan_mode(&mut self, agent_id: &str, on: bool) -> bool {
        let mut changed = false;
        let members = self
            .channels
            .list
            .iter_mut()
            .flat_map(|channel| channel.members.iter_mut());
        for member in members.filter(|member| member.id == agent_id) {
            changed |= member.plan_mode != on;
            member.plan_mode = on;
        }
        changed
    }

    /// Keep `posts` as channel `channel_id`'s, filing its `/plan` requests
    /// apart for HISTORY.
    pub(in crate::app) fn set_channel_posts(
        &mut self,
        channel_id: String,
        posts: Vec<ChannelPostView>,
    ) {
        let (plans, posts): (Vec<_>, Vec<_>) =
            posts.into_iter().partition(|post| post.planner.is_some());
        self.channels.plan_posts.insert(channel_id.clone(), plans);
        self.channels.posts.insert(channel_id, posts);
    }

    /// Add `post` to its channel, or replace the post with its `seq`; a
    /// `/plan` request is filed apart for HISTORY.
    pub(in crate::app) fn add_channel_post(&mut self, channel_id: String, post: ChannelPostView) {
        let kept = if post.planner.is_some() {
            &mut self.channels.plan_posts
        } else {
            &mut self.channels.posts
        };
        let posts = kept.entry(channel_id).or_default();
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
        self.notifications.current = Some((
            format!("Archive channel #{slug}? Press y to confirm"),
            NotificationType::Warning,
        ));
        self.pending.archive_channel = Some(id);
    }

    /// Resolve a pending channel archive with `key`. Returns `false` when
    /// nothing was pending.
    pub(in crate::app) fn handle_pending_archive_channel(&mut self, key: KeyEvent) -> bool {
        let Some(id) = self.pending.archive_channel.take() else {
            return false;
        };
        if matches!(key.code, KeyCode::Char('y' | 'Y')) {
            self.try_send_event(UiEvent::ArchiveChannel(id));
        } else {
            self.notifications.current = Some(("Cancelled".to_string(), NotificationType::Warning));
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
}
