//! Input, copying, approvals and which panels are shown.

use super::*;

impl App {
    /// Get the current input text.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input.text
    }

    /// Get the cursor position.
    #[must_use]
    pub const fn cursor_pos(&self) -> usize {
        self.input.cursor_pos
    }

    /// Get the scroll offset.
    #[must_use]
    pub const fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Messages, and the posts of the channel the agent on screen
    /// coordinates, along with the cache of the lines the open pane draws
    /// them as.
    pub(crate) fn chat_line_cache(
        &mut self,
    ) -> (
        (&VecDeque<Message>, &[ChannelPostView]),
        &mut ChatLineCache,
        GroupState<'_>,
    ) {
        let groups = GroupState {
            open: &self.chat.group_open,
            expand_all: self.chat.activity_expanded,
            turn_live: self.chat_state == AppState::Processing,
        };
        let cache = if self.terminal_mode {
            self.chat.terminal_lines.set_prompt(&self.shell_prompt);
            &mut self.chat.terminal_lines
        } else {
            &mut self.chat.chat_lines
        };
        let posts = self.channels.coordinator_posts(&self.active_agent_id);
        ((&self.messages, posts), cache, groups)
    }

    /// Whether the chat panel shows the terminal pane: the shell commands
    /// run this session, in place of the conversation.
    #[must_use]
    pub const fn terminal_pane(&self) -> bool {
        self.terminal_mode
    }

    /// Whether the open pane has nothing to show.
    #[must_use]
    pub fn pane_is_empty(&self) -> bool {
        let has_posts = !self.terminal_mode
            && !self
                .channels
                .coordinator_posts(&self.active_agent_id)
                .is_empty();
        !has_posts
            && !self
                .messages
                .iter()
                .any(|m| m.shell_run().is_some() == self.terminal_mode)
    }

    /// Whether a copy is waiting for the next frame to be drawn.
    #[must_use]
    pub const fn copy_pending(&self) -> bool {
        self.select.copy_requested || self.select.copy_pending_text.is_some()
    }

    /// Ask for the current selection to be copied after the next frame.
    /// Returns `false` when nothing is selected.
    pub fn request_copy(&mut self) -> bool {
        self.select.copy_requested = self
            .select
            .selection
            .as_ref()
            .is_some_and(|s| !s.is_empty());
        self.select.copy_requested
    }

    /// The text to copy once per copy request: text asked for outright,
    /// else the selected text in `buffer` (the frame just drawn).
    pub fn take_copy_text(&mut self, buffer: &Buffer) -> Option<String> {
        let selection = std::mem::take(&mut self.select.copy_requested);
        if let Some(text) = self.select.copy_pending_text.take() {
            return Some(text);
        }
        if !selection {
            return None;
        }
        self.select.copy_label = None;
        let text = self.select.selection.as_ref()?.text(buffer);
        (!text.is_empty()).then_some(text)
    }

    /// Report the outcome of copying `chars` characters to the clipboard.
    ///
    pub fn notify_copied(&mut self, chars: usize, result: &std::io::Result<()>) {
        self.ticks.redraw = true;
        let what = self.select.copy_label.take();
        self.log.copy_finished(result.is_ok());
        self.notifications.current = Some(match result {
            Ok(()) => (
                match what {
                    Some(what) => format!("Copied {what} ({chars} characters)"),
                    None => format!(
                        "Copied {chars} character{}",
                        if chars == 1 { "" } else { "s" }
                    ),
                },
                NotificationType::Success,
            ),
            Err(e) => (format!("Copy failed: {e}"), NotificationType::Error),
        });
    }

    /// The approval prompt on screen: the oldest call still waiting.
    #[must_use]
    pub fn pending_approval(&self) -> Option<&PendingApproval> {
        self.pending.approvals.front()
    }

    /// How many calls of the chat on screen wait for an answer.
    #[must_use]
    pub fn pending_approval_count(&self) -> usize {
        self.pending.approvals.len()
    }

    /// Get the current notification.
    #[must_use]
    pub const fn notification(&self) -> Option<&(String, NotificationType)> {
        self.notifications.current.as_ref()
    }

    /// Get which panel has focus.
    #[must_use]
    pub const fn focus(&self) -> PanelFocus {
        self.focus
    }

    /// Check if the Record panel is visible. It hides while the side
    /// panel is maximized over it, and with no agent on screen.
    #[must_use]
    pub fn record_panel_visible(&self) -> bool {
        self.panels.record_visible
            && !self.panels.chat_expanded
            && !self.side_maximized()
            && !self.no_agent_on_screen()
    }

    /// Whether the side panel, shown, takes the Record panel's room too:
    /// the user maximized it, or the channel app is on screen, which has
    /// no Record panel and whose own panel fills the right column.
    pub(super) const fn side_maximized(&self) -> bool {
        self.channel_app() || (self.panels.side_expanded && self.panels.sandbox_visible)
    }

    /// Check if the Swarm panel is visible.
    #[must_use]
    pub const fn swarm_panel_visible(&self) -> bool {
        self.panels.swarm_visible && !self.panels.chat_expanded
    }

    /// Check if the Sandbox panel is visible. While a channel is open it
    /// is the channel's panel, shown whatever the user did to the agent's
    /// unless the hamburger button hid it; it hides with no agent on
    /// screen, which leaves it nothing to show.
    #[must_use]
    pub fn sandbox_panel_visible(&self) -> bool {
        let shown = if self.channel_open() {
            !self.panels.channel_side_hidden
        } else {
            self.panels.sandbox_visible
        };
        shown && !self.panels.chat_expanded && !self.no_agent_on_screen()
    }

    /// Whether the chat panel fills the screen, the other panels hidden.
    #[must_use]
    pub const fn chat_expanded(&self) -> bool {
        self.panels.chat_expanded
    }

    /// Expand the chat panel to fill the screen, or give the other panels
    /// their room back. The chat keeps whichever tab or view it shows.
    pub fn toggle_chat_expanded(&mut self) {
        self.panels.chat_expanded = !self.panels.chat_expanded;
        if self.panels.chat_expanded {
            self.focus = PanelFocus::Chat;
        }
    }

    /// Whether the side panel fills the right column, the Record panel
    /// above it hidden.
    #[must_use]
    pub const fn side_expanded(&self) -> bool {
        self.panels.side_expanded
    }

    /// Maximize the side panel over the Record panel, focused, or put it
    /// back below it with the split it had. The channel app's panel fills
    /// the column anyway, so there this does nothing.
    pub fn toggle_side_expanded(&mut self) {
        if self.channel_app() {
            return;
        }
        self.panels.side_expanded = !self.panels.side_expanded;
        if self.panels.side_expanded {
            self.focus = PanelFocus::Sandbox;
        }
    }

    /// Whether the right column, the Record panel above the side panel,
    /// is on screen.
    #[must_use]
    pub fn side_column_shown(&self) -> bool {
        self.record_panel_visible() || self.sandbox_panel_visible()
    }

    /// Hide the right column, remembering which of its panels were shown,
    /// or show those again, as the hamburger button at the right end of
    /// the header does. With nothing remembered, or nothing to show, both
    /// come back. While the chat is expanded the column is not on screen,
    /// so the button restores the chat and shows the column.
    ///
    /// Only the panels' own flags change, so `/kernel`, `/files` and the
    /// tabs that open the Record panel show the column again as usual.
    /// In a channel the button hides or shows the channel's panel alone,
    /// leaving the agent panels' flags and what they remember as they were.
    pub fn toggle_side_column(&mut self) {
        if self.channel_open() {
            self.toggle_channel_side();
            return;
        }
        if self.side_column_shown() {
            self.panels.side_column_hidden =
                Some((self.panels.record_visible, self.panels.sandbox_visible));
            self.panels.record_visible = false;
            self.panels.sandbox_visible = false;
            if matches!(self.focus, PanelFocus::Records | PanelFocus::Sandbox) {
                self.focus = PanelFocus::Chat;
            }
            return;
        }
        self.panels.chat_expanded = false;
        let remembered = self.panels.side_column_hidden.take();
        if self.panels.record_visible || self.panels.sandbox_visible {
            // It was only under the expanded chat.
            return;
        }
        let (records, side) = remembered
            .filter(|&shown| shown != (false, false))
            .unwrap_or((true, true));
        self.panels.record_visible = records;
        self.panels.sandbox_visible = side;
        if side {
            self.sandbox.refresh();
        }
    }

    /// [`Self::toggle_side_column`] in a channel: hide the channel's panel,
    /// its room going to the chat, or show it again. While the chat is
    /// expanded the panel is not on screen, so the button restores the
    /// chat and shows the panel, as it does for the agent's column.
    fn toggle_channel_side(&mut self) {
        if self.sandbox_panel_visible() {
            self.panels.channel_side_hidden = true;
            self.keep_channel_focus_on_screen();
            return;
        }
        self.panels.chat_expanded = false;
        self.panels.channel_side_hidden = false;
    }

    /// In the channel app, move the focus to the chat when it is on a
    /// panel not on screen: the channel's panel the hamburger button hid,
    /// or the agent panels the app leaves away. Keys would otherwise move
    /// a highlight or switch a tab no one sees. Called wherever the
    /// channel's layout can change under the focus, and before each key.
    /// The agent apps keep the focus they have.
    pub(super) fn keep_channel_focus_on_screen(&mut self) {
        if self.channel_app() && !self.panel_visible(self.focus) {
            self.focus = PanelFocus::Chat;
        }
    }
}
