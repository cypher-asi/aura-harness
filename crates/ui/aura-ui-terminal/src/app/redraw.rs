//! Notifications, cancelling, ticks and redraws.

use super::*;

impl App {
    /// Clear the current notification.
    pub fn clear_notification(&mut self) {
        self.notifications.current = None;
    }

    /// How long the notification on screen has been showing; zero before
    /// its timer starts.
    #[must_use]
    pub(crate) fn notification_age(&self) -> Duration {
        self.notifications.age()
    }

    /// Keep the notification on screen until it has scrolled for `read`,
    /// and a moment after.
    pub(crate) fn hold_notification_for(&mut self, read: Duration) {
        self.notifications.hold_for(read);
    }

    /// Cancel the turn of the agent on screen.
    pub fn cancel(&mut self) {
        if self.is_busy() {
            // The event loop denies the waiting call when it cancels the
            // turn, so the prompts can go now.
            for approval in std::mem::take(&mut self.pending.approvals) {
                self.mark_awaiting_approval(&approval.id, false);
            }
            self.state = AppState::Idle;
            self.status = "Cancelled".to_string();
            if let Some(tx) = &self.link.event_tx {
                let _ = tx.try_send(UiEvent::Cancel(self.active_agent_id.clone()));
            }
        }
    }

    /// Stop the commands the agent on screen is running. Its turn goes on
    /// with what they printed.
    pub fn stop_command(&mut self) {
        if let Some(tx) = &self.link.event_tx {
            let _ = tx.try_send(UiEvent::StopCommand(self.active_agent_id.clone()));
        }
    }

    /// Add a message to the conversation.
    pub fn add_message(&mut self, message: Message) {
        self.messages.push_back(message);
        while self.messages.len() > MAX_MESSAGES {
            self.messages.pop_front();
        }
    }

    /// Send `event` to the session. Returns whether it went out.
    pub(super) fn try_send_event(&self, event: UiEvent) -> bool {
        self.link
            .event_tx
            .as_ref()
            .is_some_and(|tx| tx.try_send(event).is_ok())
    }

    /// Process pending updates from the command channel, and scroll the
    /// chat under a selection held past its top or bottom.
    pub fn tick(&mut self) {
        let shown = self.notifications.current.is_some();
        self.notifications.expire();
        if !self.selection_scrolling() {
            self.select.selection_scrolled = None;
        } else if self
            .select
            .selection_scrolled
            .is_none_or(|at| at.elapsed() >= SELECTION_SCROLL_EVERY)
        {
            self.scroll_selection();
            self.select.selection_scrolled = Some(Instant::now());
        }

        self.receive_commands();
        if self.ticks.sandbox_stale && self.panels.sandbox_visible {
            self.ticks.sandbox_stale = false;
            self.sandbox.refresh();
            self.ticks.redraw = true;
        }
        self.refresh_live_log();

        if matches!(
            self.state,
            AppState::Processing | AppState::AwaitingApproval
        ) {
            self.ticks.processing_since.get_or_insert_with(Instant::now);
        } else {
            self.ticks.processing_since = None;
        }
        let turns = if shown || self.animates_every_tick() {
            true
        } else if self.spinners_turning() {
            let due = self.ticks.spinner_wait == 0;
            self.ticks.spinner_wait = if due {
                SPINNER_TICKS - 1
            } else {
                self.ticks.spinner_wait - 1
            };
            due
        } else {
            self.ticks.spinner_wait = 0;
            false
        };
        if turns {
            self.ticks.animation_frame = self.ticks.animation_frame.wrapping_add(1);
            self.ticks.redraw = true;
        }
    }

    /// Whether the screen may look different since this was last called:
    /// a command changed what it shows, a tick turned a spinner, or
    /// something it shows changed with time. Commands for agents off
    /// screen count only when they change something on it (a
    /// notification, the projects list's marks). The host draws when this
    /// says so, and after input, which this doesn't track.
    pub fn take_redraw(&mut self) -> bool {
        std::mem::take(&mut self.ticks.redraw)
    }

    /// Have the next [`Self::take_redraw`] say the screen changed.
    pub fn request_redraw(&mut self) {
        self.ticks.redraw = true;
    }

    /// Whether the screen changes from tick to tick: a spinner turns for
    /// an agent, task or process at work, a notification scrolls, a
    /// selection held past the chat's edge scrolls it, or the chat, the
    /// LOG tab or BENCH animates.
    #[must_use]
    pub fn animating(&self) -> bool {
        self.animates_every_tick() || self.spinners_turning()
    }

    /// Whether what changes from tick to tick is more than spinner glyphs:
    /// the agent on screen at work, a notification, a selection scrolling
    /// the chat, or the chat, the LOG tab or BENCH animating.
    pub(super) fn animates_every_tick(&self) -> bool {
        self.is_busy()
            || self.notifications.current.is_some()
            || self.selection_scrolling()
            || self.bench_animating()
            || self.log_animating()
            || self.chat_animating()
    }

    /// Whether a spinner turns for an agent, task or process at work.
    pub(super) fn spinners_turning(&self) -> bool {
        self.agents.iter().any(|agent| agent.running)
            || self
                .task_list
                .tasks()
                .iter()
                .any(|task| task.state == crate::events::TaskState::Running)
            || self
                .side
                .processes
                .items
                .iter()
                .any(|process| process.status == crate::events::ProcessStatus::Running)
    }

    /// Apply every command waiting in the command channel. Between frames
    /// this keeps the channel from filling, which would hold up the agents
    /// and the event loop sending to it.
    pub fn receive_commands(&mut self) {
        if let Some(mut rx) = self.link.command_rx.take() {
            while let Ok(cmd) = rx.try_recv() {
                self.process_command(cmd);
            }
            self.link.command_rx = Some(rx);
        }
    }
}
