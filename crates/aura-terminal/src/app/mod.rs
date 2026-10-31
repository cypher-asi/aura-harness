//! Application state machine for the terminal UI.
//!
//! Manages the overall application state, conversation history,
//! panels (Chat and Record), and coordinates between input handling and rendering.

mod commands;
mod format;
mod keys;
mod shell_input;

#[cfg(test)]
mod tests;

use crate::{
    components::{Message, ToolCard, ToolStatus},
    events::{AgentSummary, ModelOptionView, RecordSummary, UiCommand, UiEvent},
    input::InputHistory,
};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::collections::VecDeque;
use std::time::{Duration, Instant};
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
    /// Model selector overlay is open
    SelectingModel,
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
    /// Models offered by the selector
    pub(super) models: Vec<ModelOptionView>,
    /// Id of the model used for the next turn
    pub(super) current_model: String,
    /// Logged-in user's display name (`None` = guest)
    pub(super) user: Option<String>,
    /// Highlighted row in the model selector
    pub(super) model_cursor: usize,
    /// Cloud model to select once the in-chat login completes
    pub(super) pending_model: Option<String>,
    /// First visible row of the agents list
    pub(super) agents_scroll: usize,
    /// Screen areas from the last frame, for mouse hit-testing
    pub(super) areas: PanelAreas,
    /// When the in-flight turn started (`None` unless processing)
    pub(super) processing_since: Option<Instant>,
}

/// Screen areas of the content panels from the last rendered frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct PanelAreas {
    /// Agents panel (including border)
    pub swarm: Option<Rect>,
    /// Rows of the agents list inside the panel
    pub agent_rows: Option<Rect>,
    /// Chat panel (including border)
    pub chat: Option<Rect>,
    /// Record panel (including border)
    pub records: Option<Rect>,
}

fn contains(area: Option<Rect>, column: u16, row: u16) -> bool {
    area.is_some_and(|a| {
        column >= a.x && column < a.x + a.width && row >= a.y && row < a.y + a.height
    })
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
            swarm_panel_visible: true,
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
            models: Vec::new(),
            current_model: String::new(),
            user: None,
            model_cursor: 0,
            pending_model: None,
            agents_scroll: 0,
            areas: PanelAreas::default(),
            processing_since: None,
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
        if let Some(tool) = self
            .tools
            .iter()
            .rev()
            .find(|t| t.status() == ToolStatus::Running)
        {
            return Some(format!("running {}", tool.name()));
        }
        if self.status.starts_with("Running") {
            return Some("running".to_string());
        }
        Some("thinking".to_string())
    }

    /// Time since the in-flight turn started.
    #[must_use]
    pub fn processing_elapsed(&self) -> Option<Duration> {
        self.processing_since.map(|since| since.elapsed())
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
        self.scroll_panel_up(self.focus, lines);
    }

    /// Scroll down by the given number of lines (panel-aware).
    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_panel_down(self.focus, lines);
    }

    fn scroll_panel_up(&mut self, panel: PanelFocus, lines: usize) {
        match panel {
            PanelFocus::Chat => {
                self.scroll_offset = self.scroll_offset.saturating_add(lines);
            }
            PanelFocus::Records => {
                self.selected_record = self.selected_record.saturating_sub(lines);
            }
            PanelFocus::Swarm => {
                self.select_agent_row(self.selected_agent.saturating_sub(lines));
            }
        }
    }

    fn scroll_panel_down(&mut self, panel: PanelFocus, lines: usize) {
        match panel {
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
                self.select_agent_row(self.selected_agent.saturating_add(lines));
            }
        }
    }

    /// Highlight agents-list row `row` (the last row is "+ New agent") and
    /// switch to the agent on it.
    pub(super) fn select_agent_row(&mut self, row: usize) {
        self.selected_agent = row.min(self.agents.len());
        if let Some(agent_id) = self.agents.get(self.selected_agent).map(|a| a.id.clone()) {
            self.request_agent_switch(agent_id);
        }
    }

    /// Act on the highlighted agents-list row: switch to it, or create an
    /// agent when it is the "+ New agent" row.
    pub(super) fn activate_agent_row(&mut self) {
        match self.agents.get(self.selected_agent).map(|a| a.id.clone()) {
            Some(agent_id) => self.request_agent_switch(agent_id),
            None => self.create_agent(),
        }
    }

    /// Create a new agent and switch to it.
    pub(super) fn create_agent(&mut self) {
        let _ = self.handle_command("/agent new");
    }

    /// Switch to the agent `delta` rows away from the active one.
    pub(super) fn switch_agent_by(&mut self, delta: isize) {
        if self.agents.is_empty() {
            return;
        }
        let current = self
            .agents
            .iter()
            .position(|a| a.id == self.active_agent_id)
            .unwrap_or(self.selected_agent);
        let last = self.agents.len() - 1;
        let target = current.saturating_add_signed(delta).min(last);
        self.select_agent_row(target);
    }

    /// First visible row of the agents list.
    #[must_use]
    pub const fn agents_scroll(&self) -> usize {
        self.agents_scroll
    }

    /// Keep the highlighted agents-list row inside a list `visible_rows` tall.
    pub fn sync_agents_scroll(&mut self, visible_rows: usize) {
        let visible_rows = visible_rows.max(1);
        let total_rows = self.agents.len() + 1;
        if self.selected_agent < self.agents_scroll {
            self.agents_scroll = self.selected_agent;
        } else if self.selected_agent >= self.agents_scroll + visible_rows {
            self.agents_scroll = self.selected_agent + 1 - visible_rows;
        }
        self.agents_scroll = self
            .agents_scroll
            .min(total_rows.saturating_sub(visible_rows));
    }

    /// Record where the panels were drawn this frame.
    pub fn set_panel_areas(&mut self, areas: PanelAreas) {
        self.areas = areas;
    }

    /// Record where the agents list rows were drawn this frame.
    pub fn set_agent_rows_area(&mut self, area: Rect) {
        self.areas.agent_rows = Some(area);
    }

    fn panel_at(&self, column: u16, row: u16) -> Option<PanelFocus> {
        if contains(self.areas.swarm, column, row) {
            Some(PanelFocus::Swarm)
        } else if contains(self.areas.records, column, row) {
            Some(PanelFocus::Records)
        } else if contains(self.areas.chat, column, row) {
            Some(PanelFocus::Chat)
        } else {
            None
        }
    }

    /// Handle a mouse event: the wheel scrolls the panel under the pointer,
    /// a left click focuses it, and clicking an agent row switches to it.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.state != AppState::Idle && self.state != AppState::Processing {
            return;
        }
        let Some(panel) = self.panel_at(mouse.column, mouse.row) else {
            return;
        };
        let step = if panel == PanelFocus::Swarm { 1 } else { 3 };
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_panel_up(panel, step),
            MouseEventKind::ScrollDown => self.scroll_panel_down(panel, step),
            MouseEventKind::Down(MouseButton::Left) => {
                self.focus = panel;
                if panel == PanelFocus::Swarm {
                    self.click_agent_row(mouse.column, mouse.row);
                }
            }
            _ => {}
        }
    }

    fn click_agent_row(&mut self, column: u16, row: u16) {
        let Some(rows) = self.areas.agent_rows else {
            return;
        };
        if !contains(Some(rows), column, row) {
            return;
        }
        let index = self.agents_scroll + usize::from(row - rows.y);
        if index > self.agents.len() {
            return;
        }
        self.selected_agent = index;
        self.activate_agent_row();
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

    /// Models offered by the selector.
    #[must_use]
    pub fn models(&self) -> &[ModelOptionView] {
        &self.models
    }

    /// Id of the model used for the next turn (empty until the kernel reports it).
    #[must_use]
    pub fn current_model(&self) -> &str {
        &self.current_model
    }

    /// Selector entry for the current model, if listed.
    #[must_use]
    pub fn current_model_option(&self) -> Option<&ModelOptionView> {
        self.models.iter().find(|m| m.id == self.current_model)
    }

    /// Logged-in user's display name (`None` = guest).
    #[must_use]
    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Highlighted row in the model selector.
    #[must_use]
    pub const fn model_cursor(&self) -> usize {
        self.model_cursor
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

    /// Request a switch to another agent.
    ///
    /// Refused while a turn is in flight, because the backend processes one
    /// agent's turn at a time and the streamed output belongs to the current agent.
    pub(super) fn request_agent_switch(&mut self, agent_id: String) {
        if matches!(
            self.state,
            AppState::Processing | AppState::AwaitingApproval
        ) {
            self.notification = Some((
                "Wait for the current turn to finish before switching agents".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        if agent_id == self.active_agent_id {
            return;
        }
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::SelectAgent(agent_id));
        }
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

        if matches!(
            self.state,
            AppState::Processing | AppState::AwaitingApproval
        ) {
            self.processing_since.get_or_insert_with(Instant::now);
        } else {
            self.processing_since = None;
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
