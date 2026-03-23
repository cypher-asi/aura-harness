//! Application state machine for the terminal UI.
//!
//! Manages the overall application state, conversation history,
//! panels (Chat and Record), and coordinates between input handling and rendering.

use crate::{
    components::{Message, MessageRole, ToolCard, ToolStatus},
    events::{AgentSummary, RecordSummary, UiCommand, UiEvent},
    input::InputHistory,
    terminal::KeyResult,
};
use crossterm::event::{KeyCode, KeyEvent};
use std::collections::VecDeque;
use tokio::sync::mpsc;
use tracing::debug;

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
    state: AppState,
    /// Conversation messages
    messages: VecDeque<Message>,
    /// Active tool cards
    tools: Vec<ToolCard>,
    /// Current input text
    input: String,
    /// Input history for navigation
    input_history: InputHistory,
    /// Cursor position in input
    cursor_pos: usize,
    /// Current status message
    status: String,
    /// Pending approval (if any)
    pending_approval: Option<PendingApproval>,
    /// Scroll offset for messages
    scroll_offset: usize,
    /// Whether verbose mode is enabled
    verbose: bool,
    /// Event sender (for sending events to kernel)
    event_tx: Option<mpsc::Sender<UiEvent>>,
    /// Command receiver (for receiving commands from kernel)
    command_rx: Option<mpsc::Receiver<UiCommand>>,
    /// Current streaming message (being built)
    streaming_content: String,
    /// Current thinking content (being built)
    thinking_content: String,
    /// Whether currently streaming thinking
    is_thinking: bool,
    /// Notification message (ephemeral)
    notification: Option<(String, NotificationType)>,
    /// Which panel has focus
    focus: PanelFocus,
    /// Whether the Record panel is visible
    record_panel_visible: bool,
    /// Whether the Swarm panel is visible
    swarm_panel_visible: bool,
    /// Animation frame counter for spinners
    animation_frame: usize,
    /// Kernel records list
    records: VecDeque<RecordSummary>,
    /// Selected record index in the list
    selected_record: usize,
    /// Scroll offset for records list
    records_scroll: usize,
    /// Whether showing record detail view
    showing_record_detail: bool,
    /// List of agents in the swarm
    agents: Vec<AgentSummary>,
    /// Selected agent index in the swarm panel
    selected_agent: usize,
    /// Currently active agent ID
    active_agent_id: String,
    /// API URL for the swarm
    api_url: Option<String>,
    /// Whether the API is currently active
    api_active: bool,
    /// Stored email during login flow (between email and password steps)
    login_email: String,
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
    /// Called by the renderer after computing the actual content height in lines.
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
                // Move selection up in records list
                self.selected_record = self.selected_record.saturating_sub(lines);
            }
            PanelFocus::Swarm => {
                // Move selection up in agents list
                self.selected_agent = self.selected_agent.saturating_sub(lines);
            }
        }
    }

    /// Scroll down by the given number of lines (panel-aware).
    pub fn scroll_down(&mut self, lines: usize) {
        match self.focus {
            PanelFocus::Chat => {
                // Scroll down means we want to see newer messages (decrease offset)
                self.scroll_offset = self.scroll_offset.saturating_sub(lines);
            }
            PanelFocus::Records => {
                // Move selection down in records list
                if !self.records.is_empty() {
                    self.selected_record =
                        (self.selected_record + lines).min(self.records.len().saturating_sub(1));
                }
            }
            PanelFocus::Swarm => {
                // Move selection down in agents list
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

    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> KeyResult {
        // Clear any notification on input
        self.notification = None;

        // Handle record detail view
        if self.showing_record_detail {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                self.showing_record_detail = false;
            }
            return KeyResult::continue_running();
        }

        match self.state {
            AppState::AwaitingApproval => self.handle_approval_key(key),
            AppState::ShowingHelp => {
                // Any key dismisses help
                self.state = AppState::Idle;
                KeyResult::continue_running()
            }
            AppState::LoginEmail | AppState::LoginPassword => self.handle_login_key(key),
            AppState::Idle | AppState::Processing => self.handle_normal_key(key),
        }
    }

    /// Handle key in approval mode.
    fn handle_approval_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                if let Some(approval) = self.pending_approval.take() {
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.try_send(UiEvent::Approve(approval.id));
                    }
                    self.state = AppState::Processing;
                    self.status = "Approved, continuing...".to_string();
                }
            }
            KeyCode::Char('n' | 'N') => {
                if let Some(approval) = self.pending_approval.take() {
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.try_send(UiEvent::Deny(approval.id));
                    }
                    self.state = AppState::Idle;
                    self.status = "Denied".to_string();
                }
            }
            KeyCode::Esc => {
                self.pending_approval = None;
                self.state = AppState::Idle;
            }
            _ => {}
        }
        KeyResult::continue_running()
    }

    /// Handle key during login flow (email or password entry).
    fn handle_login_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Esc => {
                self.state = AppState::Idle;
                self.status = "Ready".to_string();
                self.input.clear();
                self.cursor_pos = 0;
                self.login_email.clear();
                self.notification =
                    Some(("Login cancelled".to_string(), NotificationType::Warning));
            }
            KeyCode::Enter => {
                let value = std::mem::take(&mut self.input);
                self.cursor_pos = 0;

                if value.trim().is_empty() {
                    self.notification =
                        Some(("Cannot be empty".to_string(), NotificationType::Warning));
                    return KeyResult::continue_running();
                }

                match self.state {
                    AppState::LoginEmail => {
                        self.login_email = value.trim().to_string();
                        self.state = AppState::LoginPassword;
                        self.status = "Login — enter password".to_string();
                    }
                    AppState::LoginPassword => {
                        let email = std::mem::take(&mut self.login_email);
                        self.state = AppState::Processing;
                        self.status = "Authenticating...".to_string();
                        if let Some(tx) = &self.event_tx {
                            let _ = tx.try_send(UiEvent::LoginCredentials {
                                email,
                                password: value,
                            });
                        }
                    }
                    _ => {}
                }
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Left => {
                self.cursor_pos = self.cursor_pos.saturating_sub(1);
            }
            KeyCode::Right => {
                if self.cursor_pos < self.input.len() {
                    self.cursor_pos += 1;
                }
            }
            KeyCode::Home => self.cursor_pos = 0,
            KeyCode::End => self.cursor_pos = self.input.len(),
            _ => {}
        }
        KeyResult::continue_running()
    }

    /// Handle key in normal mode.
    fn handle_normal_key(&mut self, key: KeyEvent) -> KeyResult {
        // Tab switches focus between visible panels (right-to-left, then wrap)
        // Order: Chat → Record → Swarm → Chat
        if key.code == KeyCode::Tab {
            self.focus = self.next_panel_focus();
            return KeyResult::continue_running();
        }

        // Text input keys ALWAYS go to chat, regardless of focus
        // This allows typing messages even when browsing records or agents
        match key.code {
            KeyCode::Char(_)
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Home
            | KeyCode::End => {
                return self.handle_chat_key(key);
            }
            KeyCode::Enter => {
                // Enter submits chat if there's input, otherwise panel-specific action
                if !self.input.is_empty() {
                    return self.handle_chat_key(key);
                }
            }
            _ => {}
        }

        // Handle remaining keys based on which panel has focus
        match self.focus {
            PanelFocus::Chat => self.handle_chat_key(key),
            PanelFocus::Records => self.handle_records_key(key),
            PanelFocus::Swarm => self.handle_swarm_key(key),
        }
    }

    /// Get the next panel focus when Tab is pressed.
    /// Goes right-to-left: Chat → Record → Swarm → Chat
    const fn next_panel_focus(&self) -> PanelFocus {
        match self.focus {
            PanelFocus::Chat => {
                if self.record_panel_visible {
                    PanelFocus::Records
                } else if self.swarm_panel_visible {
                    PanelFocus::Swarm
                } else {
                    PanelFocus::Chat
                }
            }
            PanelFocus::Records => {
                if self.swarm_panel_visible {
                    PanelFocus::Swarm
                } else {
                    PanelFocus::Chat
                }
            }
            PanelFocus::Swarm => PanelFocus::Chat,
        }
    }

    /// Handle key when chat panel is focused.
    fn handle_chat_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Enter => {
                // Don't allow submitting while already processing
                if !self.input.is_empty() && self.state != AppState::Processing {
                    return self.submit_input();
                }
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.input.len() {
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Left => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                }
            }
            KeyCode::Right => {
                if self.cursor_pos < self.input.len() {
                    self.cursor_pos += 1;
                }
            }
            KeyCode::Home => {
                self.cursor_pos = 0;
            }
            KeyCode::End => {
                self.cursor_pos = self.input.len();
            }
            KeyCode::Up => {
                if let Some(prev) = self.input_history.previous() {
                    self.input = prev.to_string();
                    self.cursor_pos = self.input.len();
                }
            }
            KeyCode::Down => {
                if let Some(newer) = self.input_history.next_newer() {
                    self.input = newer.to_string();
                    self.cursor_pos = self.input.len();
                } else {
                    self.input.clear();
                    self.cursor_pos = 0;
                }
            }
            KeyCode::PageUp => {
                self.scroll_up(5);
            }
            KeyCode::PageDown => {
                self.scroll_down(5);
            }
            _ => {}
        }
        KeyResult::continue_running()
    }

    /// Handle key when records panel is focused.
    fn handle_records_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Up => {
                if self.selected_record > 0 {
                    self.selected_record -= 1;
                }
            }
            KeyCode::Down => {
                if !self.records.is_empty() && self.selected_record < self.records.len() - 1 {
                    self.selected_record += 1;
                }
            }
            KeyCode::Enter => {
                if !self.records.is_empty() {
                    self.showing_record_detail = true;
                }
            }
            KeyCode::PageUp => {
                self.selected_record = self.selected_record.saturating_sub(5);
            }
            KeyCode::PageDown => {
                if !self.records.is_empty() {
                    self.selected_record =
                        (self.selected_record + 5).min(self.records.len().saturating_sub(1));
                }
            }
            KeyCode::Home => {
                self.selected_record = 0;
            }
            KeyCode::End => {
                if !self.records.is_empty() {
                    self.selected_record = self.records.len() - 1;
                }
            }
            // Allow typing in chat even when records panel is focused
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            _ => {}
        }
        KeyResult::continue_running()
    }

    /// Handle key when swarm panel is focused.
    fn handle_swarm_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Up => {
                if self.selected_agent > 0 {
                    self.selected_agent -= 1;
                }
            }
            KeyCode::Down => {
                if !self.agents.is_empty() && self.selected_agent < self.agents.len() - 1 {
                    self.selected_agent += 1;
                }
            }
            KeyCode::Enter => {
                // Select this agent as active
                if let Some(agent) = self.agents.get(self.selected_agent) {
                    let agent_id = agent.id.clone();
                    if agent_id != self.active_agent_id {
                        if let Some(tx) = &self.event_tx {
                            let _ = tx.try_send(UiEvent::SelectAgent(agent_id));
                        }
                    }
                }
            }
            KeyCode::PageUp => {
                self.selected_agent = self.selected_agent.saturating_sub(5);
            }
            KeyCode::PageDown => {
                if !self.agents.is_empty() {
                    self.selected_agent =
                        (self.selected_agent + 5).min(self.agents.len().saturating_sub(1));
                }
            }
            KeyCode::Home => {
                self.selected_agent = 0;
            }
            KeyCode::End => {
                if !self.agents.is_empty() {
                    self.selected_agent = self.agents.len() - 1;
                }
            }
            // Allow typing in chat even when swarm panel is focused
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            _ => {}
        }
        KeyResult::continue_running()
    }

    /// Submit the current input.
    fn submit_input(&mut self) -> KeyResult {
        let text = std::mem::take(&mut self.input);
        self.cursor_pos = 0;
        self.input_history.add(&text);

        // Check for slash commands
        if text.starts_with('/') {
            return self.handle_command(&text);
        }

        // Regular message - add to conversation and send event
        self.add_message(Message::new(MessageRole::User, &text));
        self.state = AppState::Processing;
        self.status = "Thinking...".to_string();

        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::UserMessage(text));
        }
        KeyResult::continue_running()
    }

    /// Handle a slash command.
    fn handle_command(&mut self, text: &str) -> KeyResult {
        let parts: Vec<&str> = text[1..].splitn(2, ' ').collect();
        let cmd = parts[0].to_lowercase();
        let _arg = parts.get(1).unwrap_or(&"");

        match cmd.as_str() {
            "quit" | "exit" | "q" => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::Quit);
                }
                return KeyResult::quit();
            }
            "help" | "?" => {
                self.state = AppState::ShowingHelp;
            }
            "clear" => {
                self.messages.clear();
                self.tools.clear();
                self.scroll_offset = 0;
                self.thinking_content.clear();
                self.is_thinking = false;
            }
            "status" | "s" => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::ShowStatus);
                }
            }
            "history" | "h" => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::ShowHistory(None));
                }
            }
            "record" | "r" => {
                self.record_panel_visible = !self.record_panel_visible;
                // If closing panel while it has focus, switch to chat
                if !self.record_panel_visible && self.focus == PanelFocus::Records {
                    self.focus = PanelFocus::Chat;
                }
            }
            "swarm" | "sw" => {
                self.swarm_panel_visible = !self.swarm_panel_visible;
                // If closing panel while it has focus, switch to chat
                if !self.swarm_panel_visible && self.focus == PanelFocus::Swarm {
                    self.focus = PanelFocus::Chat;
                }
                // Request agent list refresh when opening
                if self.swarm_panel_visible {
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.try_send(UiEvent::RefreshAgents);
                    }
                }
            }
            "login" => {
                self.state = AppState::LoginEmail;
                self.login_email.clear();
                self.status = "Login — enter email".to_string();
            }
            "logout" => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::Logout);
                }
            }
            "whoami" | "me" => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::Whoami);
                }
            }
            "new" | "n" => {
                // Clear only conversation UI state (not records - they persist per agent)
                self.messages.clear();
                self.tools.clear();
                self.scroll_offset = 0;
                self.streaming_content.clear();
                self.thinking_content.clear();
                self.is_thinking = false;
                // Notify kernel to reset context
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::NewSession);
                }
                self.notification =
                    Some(("New session started".to_string(), NotificationType::Success));
            }
            _ => {
                self.notification = Some((
                    format!("Unknown command: /{cmd}. Type /help for available commands."),
                    NotificationType::Warning,
                ));
            }
        }
        KeyResult::continue_running()
    }

    /// Add a message to the conversation.
    pub fn add_message(&mut self, message: Message) {
        self.messages.push_back(message);
        while self.messages.len() > MAX_MESSAGES {
            self.messages.pop_front();
        }
        // Reset scroll to show newest
        self.scroll_offset = 0;
    }

    /// Add a tool card.
    pub fn add_tool(&mut self, tool: ToolCard) {
        self.tools.push(tool);
    }

    /// Process a UI command from the kernel.
    #[allow(clippy::too_many_lines)]
    pub fn process_command(&mut self, cmd: UiCommand) {
        debug!(?cmd, "Processing UI command");
        match cmd {
            UiCommand::SetStatus(status) => {
                self.status = status;
            }
            UiCommand::StartStreaming => {
                // Clear any existing streaming/thinking content and start fresh
                self.streaming_content.clear();
                self.thinking_content.clear();
                self.is_thinking = false;
                // Add a placeholder streaming message that we'll update
                let mut msg = Message::new(MessageRole::Assistant, "");
                msg.set_streaming(true);
                self.add_message(msg);
            }
            UiCommand::AppendText(text) => {
                self.streaming_content.push_str(&text);
                // Update the last message if it's a streaming message
                if let Some(last_msg) = self.messages.back_mut() {
                    if last_msg.is_streaming() {
                        last_msg.set_content(&self.streaming_content);
                    }
                }
            }
            UiCommand::FinishStreaming => {
                // Finalize the streaming message
                if let Some(last_msg) = self.messages.back_mut() {
                    if last_msg.is_streaming() {
                        last_msg.set_streaming(false);
                        if self.streaming_content.is_empty() {
                            // Remove empty streaming messages
                            self.messages.pop_back();
                        } else {
                            last_msg.set_content(&self.streaming_content);
                        }
                    }
                }
                self.streaming_content.clear();
            }
            UiCommand::StartThinking => {
                // Clear any existing thinking content and start fresh
                self.thinking_content.clear();
                self.is_thinking = true;
            }
            UiCommand::AppendThinking(thinking) => {
                self.thinking_content.push_str(&thinking);
            }
            UiCommand::FinishThinking => {
                // Thinking complete - content is kept for display until next response
                self.is_thinking = false;
            }
            UiCommand::ShowMessage(data) => {
                let mut msg = Message::new(data.role, &data.content);
                if data.is_streaming {
                    msg.set_streaming(true);
                }
                self.add_message(msg);
            }
            UiCommand::ShowTool(data) => {
                let tool = ToolCard::new(&data.id, &data.name).with_args(&data.args);
                self.add_tool(tool);

                // Add tool execution to conversation as a system message
                let tool_summary = format_tool_summary(&data.name, &data.args);
                self.add_message(Message::new(
                    MessageRole::System,
                    &format!("🔧 {} : {}", data.name, tool_summary),
                ));
            }
            UiCommand::CompleteTool {
                id,
                result,
                success,
            } => {
                let mut tool_name = String::new();
                for tool in &mut self.tools {
                    if tool.id() == id {
                        tool_name = tool.name().to_string();
                        tool.set_status(if success {
                            ToolStatus::Success
                        } else {
                            ToolStatus::Error
                        });
                        tool.set_result(&result);
                    }
                }

                // Add tool result to conversation
                if !result.is_empty() {
                    let (icon, prefix) = if success {
                        ("✓", "")
                    } else {
                        ("✗", "Error: ")
                    };
                    // Truncate long results for display
                    let display_result = if result.len() > 200 {
                        format!("{}...", &result[..197])
                    } else {
                        result
                    };
                    self.add_message(Message::new(
                        MessageRole::System,
                        &format!("   {icon} {tool_name}: {prefix}{display_result}"),
                    ));
                }
            }
            UiCommand::RequestApproval {
                id,
                tool,
                description,
            } => {
                self.pending_approval = Some(PendingApproval {
                    id,
                    tool,
                    description,
                });
                self.state = AppState::AwaitingApproval;
            }
            UiCommand::ShowError(msg) => {
                // Add error to conversation as a system message
                self.add_message(Message::new(
                    MessageRole::System,
                    &format!("⛔ Error: {msg}"),
                ));
                self.notification = Some((msg, NotificationType::Error));
            }
            UiCommand::ShowSuccess(msg) => {
                // Add success to conversation as a system message
                self.add_message(Message::new(MessageRole::System, &format!("✓ {msg}")));
                self.notification = Some((msg, NotificationType::Success));
            }
            UiCommand::ShowWarning(msg) => {
                // Add warning to conversation as a system message
                self.add_message(Message::new(
                    MessageRole::System,
                    &format!("⚠ Warning: {msg}"),
                ));
                self.notification = Some((msg, NotificationType::Warning));
            }
            UiCommand::Complete => {
                // Finalize any streaming message
                if let Some(last_msg) = self.messages.back_mut() {
                    if last_msg.is_streaming() {
                        last_msg.set_streaming(false);
                        if !self.streaming_content.is_empty() {
                            last_msg.set_content(&self.streaming_content);
                        } else if last_msg.content().is_empty() {
                            // Remove empty streaming messages
                            self.messages.pop_back();
                        }
                    }
                }
                self.streaming_content.clear();
                // Clear thinking state - thinking content persists for display
                // but is_thinking should be false since we're done
                self.is_thinking = false;
                self.state = AppState::Idle;
                self.status = "Ready".to_string();
                self.tools.clear();
            }
            UiCommand::ClearConversation => {
                self.messages.clear();
                self.tools.clear();
                self.thinking_content.clear();
                self.is_thinking = false;
            }
            UiCommand::NewRecord(record) => {
                self.records.push_front(record);
                while self.records.len() > MAX_RECORDS {
                    self.records.pop_back();
                }
                // Keep selection in bounds
                if self.selected_record >= self.records.len() && !self.records.is_empty() {
                    self.selected_record = self.records.len() - 1;
                }
            }
            UiCommand::SetAgents(agents) => {
                self.agents = agents;
                // Update selected agent index to match active agent
                if let Some(idx) = self
                    .agents
                    .iter()
                    .position(|a| a.id == self.active_agent_id)
                {
                    self.selected_agent = idx;
                }
                // Keep selection in bounds
                if self.selected_agent >= self.agents.len() && !self.agents.is_empty() {
                    self.selected_agent = self.agents.len() - 1;
                }
            }
            UiCommand::SetActiveAgent(agent_id) => {
                self.active_agent_id = agent_id;
                // Update is_active flag on agents
                for agent in &mut self.agents {
                    agent.is_active = agent.id == self.active_agent_id;
                }
                // Move selection to active agent
                if let Some(idx) = self.agents.iter().position(|a| a.is_active) {
                    self.selected_agent = idx;
                }
            }
            UiCommand::ClearRecords => {
                self.records.clear();
                self.selected_record = 0;
            }
            UiCommand::SetApiStatus { url, active } => {
                self.api_url = url;
                self.api_active = active;
            }
        }
    }

    /// Process pending updates from the command channel.
    pub fn tick(&mut self) {
        // Advance animation frame
        self.animation_frame = self.animation_frame.wrapping_add(1);

        // Process any pending commands from the channel
        // We need to take the receiver temporarily to avoid borrow issues
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

/// Format a tool summary from tool name and JSON args for display.
fn format_tool_summary(tool_name: &str, args_json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(args_json).map_or_else(
        |_| {
            if args_json.len() > 50 {
                format!("{}...", &args_json[..47])
            } else {
                args_json.to_string()
            }
        },
        |args| format_tool_summary_from_value(tool_name, &args),
    )
}

/// Format a tool summary given parsed JSON args.
fn format_tool_summary_from_value(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "run_command" => {
            let program = args.get("program").and_then(|v| v.as_str()).unwrap_or("");
            let cmd_args = args
                .get("args")
                .and_then(|v| {
                    v.as_array().map(|arr| {
                        arr.iter()
                            .filter_map(|a| a.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                })
                .unwrap_or_default();
            if cmd_args.is_empty() {
                program.to_string()
            } else {
                format!("{program} {cmd_args}")
            }
        }
        "read_file" => args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "write_file" => args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "fs_list" | "list_dir" => args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string(),
        "fs_search" | "search" | "glob" => args
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => {
            let summary: Vec<String> = args
                .as_object()
                .map(|obj| {
                    obj.iter()
                        .take(3)
                        .map(|(k, v)| {
                            let val_str = match v {
                                serde_json::Value::String(s) => {
                                    if s.len() > 30 {
                                        format!("{}...", &s[..27])
                                    } else {
                                        s.clone()
                                    }
                                }
                                _ => v.to_string(),
                            };
                            format!("{k}={val_str}")
                        })
                        .collect()
                })
                .unwrap_or_default();
            summary.join(", ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn test_app_creation() {
        let app = App::new();
        assert_eq!(app.state(), AppState::Idle);
        assert!(app.messages().is_empty());
    }

    #[test]
    fn test_add_message() {
        let mut app = App::new();
        app.add_message(Message::new(MessageRole::User, "Hello"));
        assert_eq!(app.messages().len(), 1);
    }

    #[test]
    fn test_input_handling() {
        let mut app = App::new();

        // Type some text
        app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::empty()));
        app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()));
        assert_eq!(app.input(), "hi");
        assert_eq!(app.cursor_pos(), 2);

        // Backspace
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty()));
        assert_eq!(app.input(), "h");
        assert_eq!(app.cursor_pos(), 1);
    }

    #[test]
    fn test_cursor_movement() {
        let mut app = App::new();
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty()));
        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty()));
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty()));

        app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::empty()));
        assert_eq!(app.cursor_pos(), 0);

        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::empty()));
        assert_eq!(app.cursor_pos(), 3);

        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::empty()));
        assert_eq!(app.cursor_pos(), 2);
    }

    #[test]
    fn test_approval_state() {
        let mut app = App::new();
        app.pending_approval = Some(PendingApproval {
            id: "test".to_string(),
            tool: "fs.write".to_string(),
            description: "Write file".to_string(),
        });
        app.state = AppState::AwaitingApproval;

        // Deny
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty()));
        assert!(app.pending_approval.is_none());
        assert_eq!(app.state(), AppState::Idle);
    }

    // ========================================================================
    // State Transitions
    // ========================================================================

    #[test]
    fn test_initial_state_is_idle() {
        let app = App::new();
        assert_eq!(app.state(), AppState::Idle);
    }

    #[test]
    fn test_state_transition_to_processing() {
        let mut app = App::new();
        app.state = AppState::Processing;
        assert_eq!(app.state(), AppState::Processing);
    }

    #[test]
    fn test_state_transition_to_awaiting_approval() {
        let mut app = App::new();
        app.pending_approval = Some(PendingApproval {
            id: "t1".to_string(),
            tool: "write_file".to_string(),
            description: "Write file".to_string(),
        });
        app.state = AppState::AwaitingApproval;
        assert_eq!(app.state(), AppState::AwaitingApproval);
        assert!(app.pending_approval.is_some());
    }

    #[test]
    fn test_state_transition_showing_help() {
        let mut app = App::new();
        app.state = AppState::ShowingHelp;
        assert_eq!(app.state(), AppState::ShowingHelp);
    }

    #[test]
    fn test_processing_to_idle() {
        let mut app = App::new();
        app.state = AppState::Processing;
        app.state = AppState::Idle;
        assert_eq!(app.state(), AppState::Idle);
    }

    // ========================================================================
    // Input handling edge cases
    // ========================================================================

    #[test]
    fn test_delete_on_empty_input() {
        let mut app = App::new();
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty()));
        assert_eq!(app.input(), "");
        assert_eq!(app.cursor_pos(), 0);
    }

    #[test]
    fn test_insert_at_cursor_position() {
        let mut app = App::new();
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty()));
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty()));
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::empty()));
        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty()));
        assert_eq!(app.input(), "abc");
    }

    #[test]
    fn test_left_at_start_stays() {
        let mut app = App::new();
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::empty()));
        assert_eq!(app.cursor_pos(), 0);
    }

    #[test]
    fn test_right_at_end_stays() {
        let mut app = App::new();
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty()));
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::empty()));
        assert_eq!(app.cursor_pos(), 1);
    }

    // ========================================================================
    // Message handling
    // ========================================================================

    #[test]
    fn test_add_many_messages_respects_limit() {
        let mut app = App::new();
        for i in 0..MAX_MESSAGES + 10 {
            app.add_message(Message::new(MessageRole::User, &format!("msg {i}")));
        }
        assert!(app.messages().len() <= MAX_MESSAGES);
    }

    #[test]
    fn test_default_status_is_ready() {
        let app = App::new();
        assert_eq!(app.status(), "Ready");
    }

    // ========================================================================
    // Panel focus
    // ========================================================================

    #[test]
    fn test_default_focus_is_chat() {
        let app = App::new();
        assert_eq!(app.focus(), PanelFocus::Chat);
    }

    #[test]
    fn test_panel_focus_equality() {
        assert_eq!(PanelFocus::Chat, PanelFocus::Chat);
        assert_ne!(PanelFocus::Chat, PanelFocus::Swarm);
        assert_ne!(PanelFocus::Chat, PanelFocus::Records);
    }

    // ========================================================================
    // Notification types
    // ========================================================================

    #[test]
    fn test_notification_types() {
        assert_eq!(NotificationType::Success, NotificationType::Success);
        assert_ne!(NotificationType::Success, NotificationType::Warning);
        assert_ne!(NotificationType::Warning, NotificationType::Error);
    }
}
