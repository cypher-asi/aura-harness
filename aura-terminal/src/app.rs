//! Application state machine for the terminal UI.
//!
//! Manages the overall application state, conversation history,
//! and coordinates between input handling and rendering.

use crate::{
    components::{Message, MessageRole, ToolCard, ToolStatus},
    events::{RecordSummary, UiCommand, UiEvent},
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
    /// Notification message (ephemeral)
    notification: Option<(String, NotificationType)>,
    /// Which panel has focus
    focus: PanelFocus,
    /// Kernel records list
    records: VecDeque<RecordSummary>,
    /// Selected record index in the list
    selected_record: usize,
    /// Scroll offset for records list
    records_scroll: usize,
    /// Whether showing record detail view
    showing_record_detail: bool,
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
            notification: None,
            focus: PanelFocus::default(),
            records: VecDeque::new(),
            selected_record: 0,
            records_scroll: 0,
            showing_record_detail: false,
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

    /// Handle key in normal mode.
    fn handle_normal_key(&mut self, key: KeyEvent) -> KeyResult {
        // Tab switches focus between panels
        if key.code == KeyCode::Tab {
            self.focus = match self.focus {
                PanelFocus::Chat => PanelFocus::Records,
                PanelFocus::Records => PanelFocus::Chat,
            };
            return KeyResult::continue_running();
        }

        // Handle keys based on which panel has focus
        match self.focus {
            PanelFocus::Chat => self.handle_chat_key(key),
            PanelFocus::Records => self.handle_records_key(key),
        }
    }

    /// Handle key when chat panel is focused.
    fn handle_chat_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Enter => {
                if !self.input.is_empty() {
                    self.submit_input();
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
                self.scroll_offset = self.scroll_offset.saturating_add(5);
            }
            KeyCode::PageDown => {
                self.scroll_offset = self.scroll_offset.saturating_sub(5);
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

    /// Submit the current input.
    fn submit_input(&mut self) {
        let text = std::mem::take(&mut self.input);
        self.cursor_pos = 0;
        self.input_history.add(&text);

        // Check for slash commands
        if text.starts_with('/') {
            self.handle_command(&text);
            return;
        }

        // Regular message - add to conversation and send event
        self.add_message(Message::new(MessageRole::User, &text));
        self.state = AppState::Processing;
        self.status = "Thinking...".to_string();

        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::UserMessage(text));
        }
    }

    /// Handle a slash command.
    fn handle_command(&mut self, text: &str) {
        let parts: Vec<&str> = text[1..].splitn(2, ' ').collect();
        let cmd = parts[0].to_lowercase();
        let _arg = parts.get(1).unwrap_or(&"");

        match cmd.as_str() {
            "quit" | "exit" | "q" => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::Quit);
                }
            }
            "help" | "?" => {
                self.state = AppState::ShowingHelp;
            }
            "clear" => {
                self.messages.clear();
                self.tools.clear();
                self.scroll_offset = 0;
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
            _ => {
                self.notification = Some((
                    format!("Unknown command: /{cmd}. Type /help for available commands."),
                    NotificationType::Warning,
                ));
            }
        }
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
    pub fn process_command(&mut self, cmd: UiCommand) {
        debug!(?cmd, "Processing UI command");
        match cmd {
            UiCommand::SetStatus(status) => {
                self.status = status;
            }
            UiCommand::AppendText(text) => {
                self.streaming_content.push_str(&text);
            }
            UiCommand::ShowMessage(data) => {
                let role = match data.role {
                    crate::events::MessageRole::User => MessageRole::User,
                    crate::events::MessageRole::Assistant => MessageRole::Assistant,
                    crate::events::MessageRole::System => MessageRole::System,
                };
                let mut msg = Message::new(role, &data.content);
                if data.is_streaming {
                    msg.set_streaming(true);
                }
                self.add_message(msg);
            }
            UiCommand::ShowTool(data) => {
                let tool = ToolCard::new(&data.id, &data.name).with_args(&data.args);
                self.add_tool(tool);
            }
            UiCommand::CompleteTool {
                id,
                result,
                success,
            } => {
                for tool in &mut self.tools {
                    if tool.id() == id {
                        tool.set_status(if success {
                            ToolStatus::Success
                        } else {
                            ToolStatus::Error
                        });
                        tool.set_result(&result);
                    }
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
                self.notification = Some((msg, NotificationType::Error));
            }
            UiCommand::ShowSuccess(msg) => {
                self.notification = Some((msg, NotificationType::Success));
            }
            UiCommand::ShowWarning(msg) => {
                self.notification = Some((msg, NotificationType::Warning));
            }
            UiCommand::Complete => {
                // Finalize streaming message
                if !self.streaming_content.is_empty() {
                    let content = std::mem::take(&mut self.streaming_content);
                    self.add_message(Message::new(MessageRole::Assistant, &content));
                }
                self.state = AppState::Idle;
                self.status = "Ready".to_string();
                self.tools.clear();
            }
            UiCommand::ClearConversation => {
                self.messages.clear();
                self.tools.clear();
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
        }
    }

    /// Process pending updates from the command channel.
    pub fn tick(&mut self) {
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
}
