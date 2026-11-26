//! App builders, state getters and what the agent is working on.

use super::*;

impl App {
    /// Get the user's attention according to `settings`.
    #[must_use]
    pub const fn with_alert_settings(mut self, settings: AlertSettings) -> Self {
        self.ticks.alert_settings = settings;
        self
    }

    /// When the terminal gets the user's attention.
    #[must_use]
    pub const fn alert_settings(&self) -> &AlertSettings {
        &self.ticks.alert_settings
    }

    /// Messages to get the user's attention for since the last call.
    pub fn take_alerts(&mut self) -> Vec<String> {
        std::mem::take(&mut self.ticks.alerts)
    }

    /// Set the event sender for communication with kernel.
    #[must_use]
    pub fn with_event_sender(mut self, tx: mpsc::Sender<UiEvent>) -> Self {
        self.link.event_tx = Some(tx);
        self
    }

    /// Set the command receiver for communication from kernel.
    #[must_use]
    pub fn with_command_receiver(mut self, rx: mpsc::Receiver<UiCommand>) -> Self {
        self.link.command_rx = Some(rx);
        self
    }

    /// Enable or disable verbose mode.
    pub fn set_verbose(&mut self, verbose: bool) {
        self.ticks.verbose = verbose;
    }

    /// Get whether verbose mode is enabled.
    #[must_use]
    pub const fn verbose(&self) -> bool {
        self.ticks.verbose
    }

    /// Get the current application state.
    #[must_use]
    pub const fn state(&self) -> AppState {
        self.state
    }

    /// Check if currently processing a request.
    #[must_use]
    pub fn is_processing(&self) -> bool {
        self.state == AppState::Processing
    }

    /// Whether the agent on screen has a turn in flight, including one
    /// waiting for an approval.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        matches!(
            self.state,
            AppState::Processing | AppState::AwaitingApproval
        )
    }

    /// Get the current status message.
    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    /// What the agent is doing during a turn while no reply text is arriving
    /// (`"thinking"`, `"running fs.read"`, ...). `None` when idle, awaiting
    /// approval, or while reply text is streaming into the chat.
    #[must_use]
    pub fn working_label(&self) -> Option<String> {
        if self.state != AppState::Processing {
            return None;
        }
        let reply_streaming = self
            .messages
            .back()
            .is_some_and(|m| m.is_streaming() && !m.content().is_empty());
        if reply_streaming {
            return None;
        }
        if let Some(run) = self.running_shell() {
            let program = run.command.split_whitespace().next().unwrap_or_default();
            return Some(format!("running {program}"));
        }
        if let Some(tool) = self.running_tool() {
            return Some(format!("running {}", tool.name));
        }
        if self.status.starts_with("Running") {
            return Some("running".to_string());
        }
        Some("thinking".to_string())
    }

    /// How long the work [`Self::working_label`] names has gone on: the
    /// running tool call's own time, else the turn's. With it, how long a
    /// running command has printed nothing, once that is long enough to
    /// suggest it is stuck.
    #[must_use]
    pub fn working_times(&self) -> (Option<Duration>, Option<Duration>) {
        if let Some(run) = self.running_shell() {
            let quiet = quiet_for(self.ticks.processing_since, &run.live);
            return (self.processing_elapsed(), quiet);
        }
        if let Some(tool) = self.running_tool() {
            let elapsed = tool.started.map(|since| since.elapsed());
            return (
                elapsed.or_else(|| self.processing_elapsed()),
                tool.quiet_for(),
            );
        }
        (self.processing_elapsed(), None)
    }

    /// The shell command the user typed, while it runs.
    pub(super) fn running_shell(&self) -> Option<&ShellRun> {
        self.messages
            .back()
            .filter(|m| m.is_running_shell())
            .and_then(Message::shell_run)
    }

    /// The latest step still in progress, when it is a tool call.
    pub(super) fn running_tool(&self) -> Option<&ToolStep> {
        match self
            .messages
            .iter()
            .rev()
            .filter_map(Message::activity_step)
            .find(|a| a.is_running())
        {
            Some(Activity::Tool(tool)) => Some(tool),
            _ => None,
        }
    }

    /// Time since the in-flight turn started.
    #[must_use]
    pub fn processing_elapsed(&self) -> Option<Duration> {
        self.ticks.processing_since.map(|since| since.elapsed())
    }

    /// Usage, todos and jobs of the agent on screen.
    #[must_use]
    pub const fn hud(&self) -> &Hud {
        &self.hud
    }

    /// Whether the full todo list is shown above the input.
    #[must_use]
    pub const fn todos_open(&self) -> bool {
        self.todos.open
    }

    /// Whether the chat's live to-do checklist lists every item in full,
    /// rather than the first few cut to one row each.
    #[must_use]
    pub const fn todos_expanded(&self) -> bool {
        self.todos.expanded
    }

    /// Get the messages.
    #[must_use]
    pub const fn messages(&self) -> &VecDeque<Message> {
        &self.messages
    }

    /// Whether an activity block (thinking, a tool call) is in progress.
    #[must_use]
    pub fn has_live_activity(&self) -> bool {
        self.messages.iter().rev().any(Message::is_live_activity)
    }

    /// Whether new activity blocks start expanded.
    #[must_use]
    pub const fn activity_expanded(&self) -> bool {
        self.chat.activity_expanded
    }

    /// Expand every activity block, or collapse them all if they were
    /// expanded. New blocks follow suit.
    pub fn toggle_all_activity(&mut self) {
        self.chat.activity_expanded = !self.chat.activity_expanded;
        let expanded = self.chat.activity_expanded;
        self.chat.group_open.clear();
        for message in &mut self.messages {
            message.set_expanded(expanded);
        }
    }

    /// Whether the conversation ends in a group of steps that the turn in
    /// flight may still add to. Its summary line spins while it grows.
    #[must_use]
    pub fn ends_in_live_group(&self) -> bool {
        self.state == AppState::Processing
            && self.messages.len() >= 2
            && self
                .messages
                .iter()
                .rev()
                .take(2)
                .all(|m| m.activity_step().is_some())
    }
}
