//! Application state machine for the terminal UI.
//!
//! Manages the overall application state, conversation history,
//! panels (Chat and Record), and coordinates between input handling and rendering.

mod commands;
mod format;
mod keys;

#[cfg(test)]
mod tests;

use crate::{
    components::{Message, ToolCard},
    events::{AgentSummary, RecordSummary, UiCommand, UiEvent},
    input::InputHistory,
};
use std::collections::VecDeque;
use tokio::sync::mpsc;

/// Maximum number of messages to keep in history.
const MAX_MESSAGES: usize = 100;

/// Maximum number of records to keep in the list.
const MAX_RECORDS: usize = 100;

/// Which panel has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelFocus {
    /// Swarm panel (agent list)
    Swarm,
    /// Chat panel (default)
    #[default]
    Chat,
    /// Records panel
    Records,
}

/// Application state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    /// Ready for input
    Idle,
    /// Processing a request (model thinking)
    Processing,
    /// Waiting for user approval
    AwaitingApproval,
    /// Displaying help
    ShowingHelp,
    /// Login flow: waiting for email
    LoginEmail,
    /// Login flow: waiting for password
    LoginPassword,
}

/// Pending approval request.
#[derive(Debug, Clone)]
pub struct PendingApproval {
    /// Tool use ID
    pub id: String,
    /// Tool name
    pub tool: String,
    /// Description of the action
    pub description: String,
}

/// Main application struct managing UI state.
#[allow(clippy::struct_excessive_bools)]
pub struct App {
    /// Current application state
    pub(super) state: AppState,
    /// Conversation messages
    pub(super) messages: VecDeque<Message>,
    /// Active tool cards
    pub(super) tools: Vec<ToolCard>,
    /// Current input text
    pub(super) input: String,
    /// Input history for navigation
    pub(super) input_history: InputHistory,
    /// Cursor position in input
    pub(super) cursor_pos: usize,
    /// Current status message
    pub(super) status: String,
    /// Pending approval (if any)
    pub(super) pending_approval: Option<PendingApproval>,
    /// Scroll offset for messages
    pub(super) scroll_offset: usize,
    /// Whether verbose mode is enabled
    pub(super) verbose: bool,
    /// Event sender (for sending events to kernel)
    pub(super) event_tx: Option<mpsc::Sender<UiEvent>>,
    /// Command receiver (for receiving commands from kernel)
    pub(super) command_rx: Option<mpsc::Receiver<UiCommand>>,
    /// Current streaming message (being built)
    pub(super) streaming_content: String,
    /// Current thinking content (being built)
    pub(super) thinking_content: String,
    /// Whether currently streaming thinking
    pub(super) is_thinking: bool,
    /// Notification message (ephemeral)
    pub(super) notification: Option<(String, NotificationType)>,
    /// Which panel has focus
    pub(super) focus: PanelFocus,
    /// Whether the Record panel is visible
    pub(super) record_panel_visible: bool,
    /// Whether the Swarm panel is visible
    pub(super) swarm_panel_visible: bool,
    /// Animation frame counter for spinners
    pub(super) animation_frame: usize,
    /// Kernel records list
    pub(super) records: VecDeque<RecordSummary>,
    /// Selected record index in the list
    pub(super) selected_record: usize,
    /// Scroll offset for records list
    pub(super) records_scroll: usize,
    /// Whether showing record detail view
    pub(super) showing_record_detail: bool,
    /// List of agents in the swarm
    pub(super) agents: Vec<AgentSummary>,
    /// Selected agent index in the swarm panel
    pub(super) selected_agent: usize,
    /// Currently active agent ID
    pub(super) active_agent_id: String,
    /// API URL for the swarm
    pub(super) api_url: Option<String>,
    /// Whether the API is currently active
    pub(super) api_active: bool,
    /// Stored email during login flow (between email and password steps)
    pub(super) login_email: String,
}

/// Type of notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationType {
    /// Success notification
    Success,
    /// Warning notification
    Warning,
    /// Error notification
    Error,
}

impl App {
    /// Create a new application instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: AppState::Idle,
            messages: VecDeque::new(),
            tools: Vec::new(),
            input: String::new(),
            input_history: InputHistory::new(),
            cursor_pos: 0,
            status: "Ready".to_string(),
            pending_approval: None,
            scroll_offset: 0,
            verbose: false,
            event_tx: None,
            command_rx: None,
            streaming_content: String::new(),
            thinking_content: String::new(),
            is_thinking: false,
            notification: None,
            focus: PanelFocus::default(),
            record_panel_visible: true,
            swarm_panel_visible: false,
            animation_frame: 0,
            records: VecDeque::new(),
            selected_record: 0,
            records_scroll: 0,
            showing_record_detail: false,
            agents: Vec::new(),
            selected_agent: 0,
            active_agent_id: String::new(),
            api_url: None,
            api_active: false,
            login_email: String::new(),
        }
    }

    /// Set the event sender for communication with kernel.
    #[must_use]
    pub fn with_event_sender(mut self, tx: mpsc::Sender<UiEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Set the command receiver for communication from kernel.
    #[must_use]
    pub fn with_command_receiver(mut self, rx: mpsc::Receiver<UiCommand>) -> Self {
        self.command_rx = Some(rx);
        self
    }

    /// Enable or disable verbose mode.
    pub fn set_verbose(&mut self, verbose: bool) {
        self.verbose = verbose;
    }

    /// Get whether verbose mode is enabled.
    #[must_use]
    pub const fn verbose(&self) -> bool {
        self.verbose
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

    /// Get the current status message.
    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Get the messages.
    #[must_use]
    pub const fn messages(&self) -> &VecDeque<Message> {
        &self.messages
    }

    /// Get the current thinking content.
    #[must_use]
    pub fn thinking_content(&self) -> &str {
        &self.thinking_content
    }

    /// Check if currently streaming thinking.
    #[must_use]
    pub const fn is_thinking(&self) -> bool {
        self.is_thinking
    }

    /// Get the active tool cards.
    #[must_use]
    pub fn tools(&self) -> &[ToolCard] {
        &self.tools
    }

    /// Get the current input text.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Get the cursor position.
    #[must_use]
    pub const fn cursor_pos(&self) -> usize {
        self.cursor_pos
    }

    /// Get the scroll offset.
    #[must_use]
    pub const fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Clamp the scroll offset to a maximum value.
    pub fn clamp_scroll(&mut self, max: usize) {
        self.scroll_offset = self.scroll_offset.min(max);
    }

    /// Scroll up by the given number of lines (panel-aware).
    pub fn scroll_up(&mut self, lines: usize) {
        match self.focus {
            PanelFocus::Chat => {
                self.scroll_offset = self.scroll_offset.saturating_add(lines);
            }
            PanelFocus::Records => {
                self.selected_record = self.selected_record.saturating_sub(lines);
            }
            PanelFocus::Swarm => {
                self.selected_agent = self.selected_agent.saturating_sub(lines);
            }
        }
    }

    /// Scroll down by the given number of lines (panel-aware).
    pub fn scroll_down(&mut self, lines: usize) {
        match self.focus {
            PanelFocus::Chat => {
                self.scroll_offset = self.scroll_offset.saturating_sub(lines);
            }
            PanelFocus::Records => {
                if !self.records.is_empty() {
                    self.selected_record =
                        (self.selected_record + lines).min(self.records.len().saturating_sub(1));
                }
            }
            PanelFocus::Swarm => {
                if !self.agents.is_empty() {
                    self.selected_agent =
                        (self.selected_agent + lines).min(self.agents.len().saturating_sub(1));
                }
            }
        }
    }

    /// Get the pending approval.
    #[must_use]
    pub const fn pending_approval(&self) -> Option<&PendingApproval> {
        self.pending_approval.as_ref()
    }

    /// Get the current notification.
    #[must_use]
    pub const fn notification(&self) -> Option<&(String, NotificationType)> {
        self.notification.as_ref()
    }

    /// Get which panel has focus.
    #[must_use]
    pub const fn focus(&self) -> PanelFocus {
        self.focus
    }

    /// Check if the Record panel is visible.
    #[must_use]
    pub const fn record_panel_visible(&self) -> bool {
        self.record_panel_visible
    }

    /// Check if the Swarm panel is visible.
    #[must_use]
    pub const fn swarm_panel_visible(&self) -> bool {
        self.swarm_panel_visible
    }

    /// Get the list of agents.
    #[must_use]
    pub fn agents(&self) -> &[AgentSummary] {
        &self.agents
    }

    /// Get the selected agent index.
    #[must_use]
    pub const fn selected_agent(&self) -> usize {
        self.selected_agent
    }

    /// Get the active agent ID.
    #[must_use]
    pub fn active_agent_id(&self) -> &str {
        &self.active_agent_id
    }

    /// Get the API URL (if set).
    #[must_use]
    pub fn api_url(&self) -> Option<&str> {
        self.api_url.as_deref()
    }

    /// Check if the API is active.
    #[must_use]
    pub const fn api_active(&self) -> bool {
        self.api_active
    }

    /// Get the current spinner character for animations.
    #[must_use]
    pub fn spinner_char(&self) -> &'static str {
        const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        SPINNER_FRAMES[self.animation_frame % SPINNER_FRAMES.len()]
    }

    /// Get the records list.
    #[must_use]
    pub const fn records(&self) -> &VecDeque<RecordSummary> {
        &self.records
    }

    /// Get the selected record index.
    #[must_use]
    pub const fn selected_record(&self) -> usize {
        self.selected_record
    }

    /// Get the records scroll offset.
    #[must_use]
    pub const fn records_scroll(&self) -> usize {
        self.records_scroll
    }

    /// Check if showing record detail view.
    #[must_use]
    pub const fn showing_record_detail(&self) -> bool {
        self.showing_record_detail
    }

    /// Get the currently selected record (if any).
    #[must_use]
    pub fn selected_record_data(&self) -> Option<&RecordSummary> {
        self.records.get(self.selected_record)
    }

    /// Clear the current notification.
    pub fn clear_notification(&mut self) {
        self.notification = None;
    }

    /// Cancel the current operation.
    pub fn cancel(&mut self) {
        if self.state == AppState::Processing {
            self.state = AppState::Idle;
            self.status = "Cancelled".to_string();
            if let Some(tx) = &self.event_tx {
                let _ = tx.try_send(UiEvent::Cancel);
            }
        }
    }

    /// Add a message to the conversation.
    pub fn add_message(&mut self, message: Message) {
        self.messages.push_back(message);
        while self.messages.len() > MAX_MESSAGES {
            self.messages.pop_front();
        }
        self.scroll_offset = 0;
    }

    /// Add a tool card.
    pub fn add_tool(&mut self, tool: ToolCard) {
        self.tools.push(tool);
    }

    /// Process pending updates from the command channel.
    pub fn tick(&mut self) {
        self.animation_frame = self.animation_frame.wrapping_add(1);

        if let Some(mut rx) = self.command_rx.take() {
            while let Ok(cmd) = rx.try_recv() {
                self.process_command(cmd);
            }
            self.command_rx = Some(rx);
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
