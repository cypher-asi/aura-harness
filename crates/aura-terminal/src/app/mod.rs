//! Application state machine for the terminal UI.
//!
//! Manages the overall application state, conversation history,
//! panels (Chat, Record and Sandbox), and coordinates between input handling and rendering.

mod commands;
mod definitions;
mod format;
mod keys;
mod line_edit;
pub mod sandbox;
mod selection;
pub(crate) mod shell_input;
mod shell_prompt;
mod slash_menu;

#[cfg(test)]
mod tests;

use crate::{
    components::{Message, ToolCard, ToolStatus},
    events::{
        AgentDefinitionSummary, AgentSummary, ModelOptionView, RecordSummary, UiCommand, UiEvent,
    },
    input::InputHistory,
    renderer::ChatLineCache,
};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use sandbox::SandboxTree;
pub use selection::Selection;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Maximum number of messages to keep in history.
const MAX_MESSAGES: usize = 100;

/// Terminal width given to commands run before the chat is first drawn.
const DEFAULT_COMMAND_COLUMNS: u16 = 80;

/// Narrowest terminal a command is given.
const MIN_COMMAND_COLUMNS: u16 = 20;

/// How long typed input must sit unchanged before it is shown as a shell
/// command, so a half-typed word doesn't flash green while passing through a
/// program name (`w` on the way to `what`).
const COMMAND_HIGHLIGHT_DELAY: Duration = Duration::from_millis(300);

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
    /// Sandbox panel (file tree)
    Sandbox,
    /// Permanent agents panel (below the agent list)
    Definitions,
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
    /// Chat lines below the bottom of the chat view (0 = following the end)
    pub(super) scroll_offset: usize,
    /// Rendered lines of each message, reused between frames
    pub(super) chat_lines: ChatLineCache,
    /// Total chat lines in the last frame
    pub(super) chat_line_count: usize,
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
    /// Whether the Sandbox panel is visible
    pub(super) sandbox_panel_visible: bool,
    /// File tree of the active agent's sandbox
    pub(super) sandbox: SandboxTree,
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
    /// First visible row of the sandbox tree
    pub(super) sandbox_scroll: usize,
    /// Screen areas from the last frame, for mouse hit-testing
    pub(super) areas: PanelAreas,
    /// When the in-flight turn started (`None` unless processing)
    pub(super) processing_since: Option<Instant>,
    /// `[user@host dir]$ ` shown before the input line
    pub(super) shell_prompt: String,
    /// Width of the chat text area in the last frame (0 before the first)
    pub(super) chat_width: u16,
    /// Whether each program name typed so far is on `PATH`
    pub(super) path_lookups: HashMap<String, bool>,
    /// Whether the input is shown as a shell command
    pub(super) input_is_command: bool,
    /// Input as of the last [`Self::sync_input_kind`]
    pub(super) input_kind_text: String,
    /// When `input_kind_text` last changed
    pub(super) input_edited_at: Instant,
    /// Highlighted row in the slash menu
    pub(super) slash_cursor: usize,
    /// Whether Esc closed the slash menu (until the input is edited)
    pub(super) slash_dismissed: bool,
    /// Text selected with the mouse, in screen cells
    pub(super) selection: Option<Selection>,
    /// Whether the left button is still held for `selection`
    pub(super) selecting: bool,
    /// Whether the selection should be copied after the next frame
    pub(super) copy_requested: bool,
    /// Permanent agents
    pub(super) definitions: Vec<AgentDefinitionSummary>,
    /// Highlighted row in the Permanent agents panel
    pub(super) selected_definition: usize,
    /// First visible row of the Permanent agents panel
    pub(super) definitions_scroll: usize,
    /// File to open in the editor before the next frame
    pub(super) editor_request: Option<PathBuf>,
    /// Permanent agent waiting for `y` to confirm its deletion
    pub(super) pending_delete: Option<String>,
}

/// Screen areas of the content panels from the last rendered frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct PanelAreas {
    /// Agents panel (including border)
    pub swarm: Option<Rect>,
    /// Rows of the agents list inside the panel
    pub agent_rows: Option<Rect>,
    /// Permanent agents panel (including border)
    pub definitions: Option<Rect>,
    /// Rows of the permanent agents list inside the panel
    pub definition_rows: Option<Rect>,
    /// Chat panel (including border)
    pub chat: Option<Rect>,
    /// Message text area inside the chat panel
    pub chat_text: Option<Rect>,
    /// Record panel (including border)
    pub records: Option<Rect>,
    /// Sandbox panel (including border)
    pub sandbox: Option<Rect>,
    /// Rows of the sandbox tree inside the panel
    pub sandbox_rows: Option<Rect>,
    /// Tree row shown at the top of `sandbox_rows`
    pub sandbox_scroll: usize,
}

fn contains(area: Option<Rect>, column: u16, row: u16) -> bool {
    area.is_some_and(|a| {
        column >= a.x && column < a.x + a.width && row >= a.y && row < a.y + a.height
    })
}

/// First row to show of a `total`-row list in a `visible`-row view so that
/// row `selected` stays in view, moving as little as possible from `scroll`.
pub(super) fn follow_selection(
    scroll: usize,
    selected: usize,
    visible: usize,
    total: usize,
) -> usize {
    let visible = visible.max(1);
    let scroll = if selected < scroll {
        selected
    } else if selected >= scroll + visible {
        selected + 1 - visible
    } else {
        scroll
    };
    scroll.min(total.saturating_sub(visible))
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
            chat_lines: ChatLineCache::default(),
            chat_line_count: 0,
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
            sandbox_panel_visible: true,
            sandbox: SandboxTree::default(),
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
            sandbox_scroll: 0,
            areas: PanelAreas::default(),
            processing_since: None,
            shell_prompt: shell_prompt::detect(),
            chat_width: 0,
            path_lookups: HashMap::new(),
            input_is_command: false,
            input_kind_text: String::new(),
            input_edited_at: Instant::now(),
            slash_cursor: 0,
            slash_dismissed: false,
            selection: None,
            selecting: false,
            copy_requested: false,
            definitions: Vec::new(),
            selected_definition: 0,
            definitions_scroll: 0,
            editor_request: None,
            pending_delete: None,
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
        if let Some(run) = self
            .messages
            .back()
            .filter(|m| m.is_running_shell())
            .and_then(Message::shell_run)
        {
            let program = run.command.split_whitespace().next().unwrap_or_default();
            return Some(format!("running {program}"));
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

    /// Messages along with the cache of the lines they are drawn as.
    pub(crate) fn chat_line_cache(&mut self) -> (&VecDeque<Message>, &mut ChatLineCache) {
        (&self.messages, &mut self.chat_lines)
    }

    /// Fit the chat scroll position to a conversation `total_lines` long shown
    /// `visible_rows` at a time, returning the first line to draw.
    ///
    /// While scrolled up, lines added at the bottom push the offset up by as
    /// much, so the lines on screen stay put instead of sliding away under
    /// streaming output. `reflowed` means every line moved (the width
    /// changed), so there is nothing to keep in place.
    pub fn sync_chat_scroll(
        &mut self,
        total_lines: usize,
        visible_rows: usize,
        reflowed: bool,
    ) -> usize {
        if self.scroll_offset > 0 && !reflowed {
            self.scroll_offset = self
                .scroll_offset
                .saturating_add(total_lines)
                .saturating_sub(self.chat_line_count);
        }
        self.chat_line_count = total_lines;
        let bottom_start = total_lines.saturating_sub(visible_rows);
        self.scroll_offset = self.scroll_offset.min(bottom_start);
        bottom_start - self.scroll_offset
    }

    /// Lines PageUp/PageDown move the chat: a screenful, less two lines
    /// kept for context.
    pub(super) fn chat_page_rows(&self) -> usize {
        self.areas
            .chat_text
            .map_or(5, |area| usize::from(area.height).saturating_sub(2).max(1))
    }

    /// Scroll the chat to the first message.
    pub(super) fn scroll_chat_to_top(&mut self) {
        self.scroll_offset = usize::MAX;
    }

    /// Scroll the chat to the end and keep following new output.
    pub(super) fn scroll_chat_to_bottom(&mut self) {
        self.scroll_offset = 0;
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
            PanelFocus::Definitions => {
                self.select_definition_row(self.selected_definition.saturating_sub(lines));
            }
            PanelFocus::Sandbox => {
                self.sandbox
                    .move_by(-isize::try_from(lines).unwrap_or(isize::MAX));
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
            PanelFocus::Definitions => {
                self.select_definition_row(self.selected_definition.saturating_add(lines));
            }
            PanelFocus::Sandbox => {
                self.sandbox
                    .move_by(isize::try_from(lines).unwrap_or(isize::MAX));
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
        self.agents_scroll = follow_selection(
            self.agents_scroll,
            self.selected_agent,
            visible_rows,
            self.agents.len() + 1,
        );
    }

    /// Keep the selected record inside a list `visible_rows` tall, returning
    /// the first record to show.
    pub fn sync_records_scroll(&mut self, visible_rows: usize) -> usize {
        self.records_scroll = follow_selection(
            self.records_scroll,
            self.selected_record,
            visible_rows,
            self.records.len(),
        );
        self.records_scroll
    }

    /// Keep the selected sandbox row inside a tree `visible_rows` tall,
    /// returning the first row to show.
    pub fn sync_sandbox_scroll(&mut self, visible_rows: usize) -> usize {
        self.sandbox_scroll = follow_selection(
            self.sandbox_scroll,
            self.sandbox.selected(),
            visible_rows,
            self.sandbox.rows().len(),
        );
        self.sandbox_scroll
    }

    /// Record where the panels were drawn this frame.
    pub fn set_panel_areas(&mut self, areas: PanelAreas) {
        self.areas = areas;
    }

    /// Record where the chat messages were drawn this frame.
    pub fn set_chat_text_area(&mut self, area: Rect) {
        self.areas.chat_text = Some(area);
    }

    /// Record where the agents list rows were drawn this frame.
    pub fn set_agent_rows_area(&mut self, area: Rect) {
        self.areas.agent_rows = Some(area);
    }

    /// Record where the sandbox tree rows were drawn this frame and which
    /// tree row is at the top.
    pub fn set_sandbox_rows_area(&mut self, area: Rect, scroll: usize) {
        self.areas.sandbox_rows = Some(area);
        self.areas.sandbox_scroll = scroll;
    }

    fn panel_at(&self, column: u16, row: u16) -> Option<PanelFocus> {
        if contains(self.areas.swarm, column, row) {
            Some(PanelFocus::Swarm)
        } else if contains(self.areas.definitions, column, row) {
            Some(PanelFocus::Definitions)
        } else if contains(self.areas.records, column, row) {
            Some(PanelFocus::Records)
        } else if contains(self.areas.sandbox, column, row) {
            Some(PanelFocus::Sandbox)
        } else if contains(self.areas.chat, column, row) {
            Some(PanelFocus::Chat)
        } else {
            None
        }
    }

    /// Handle a mouse event: the wheel scrolls the panel under the pointer,
    /// a left click focuses it, clicking an agent row switches to it, and
    /// clicking a sandbox row selects it and opens or closes a directory.
    /// Dragging with the left button selects text inside the panel the
    /// drag started in and copies it when the button is released.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.state != AppState::Idle && self.state != AppState::Processing {
            return;
        }
        let at = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(selection) = self.selection.as_mut().filter(|_| self.selecting) {
                    selection.extend_to(at);
                }
                return;
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if std::mem::take(&mut self.selecting) {
                    if self.selection.is_some_and(|s| !s.is_empty()) {
                        self.copy_requested = true;
                    } else {
                        self.selection = None;
                    }
                }
                return;
            }
            _ => {}
        }
        let Some(panel) = self.panel_at(mouse.column, mouse.row) else {
            return;
        };
        let step = if matches!(panel, PanelFocus::Swarm | PanelFocus::Definitions) {
            1
        } else {
            3
        };
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.clear_selection();
                self.scroll_panel_up(panel, step);
            }
            MouseEventKind::ScrollDown => {
                self.clear_selection();
                self.scroll_panel_down(panel, step);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.focus = panel;
                self.selection = self
                    .selectable_area(panel)
                    .map(|bounds| Selection::new(bounds, at));
                self.selecting = self.selection.is_some();
                match panel {
                    PanelFocus::Swarm => self.click_agent_row(mouse.column, mouse.row),
                    PanelFocus::Definitions => {
                        self.click_definition_row(mouse.column, mouse.row);
                    }
                    PanelFocus::Sandbox => self.click_sandbox_row(mouse.column, mouse.row),
                    PanelFocus::Chat | PanelFocus::Records => {}
                }
            }
            _ => {}
        }
    }

    /// Cells of `panel` that hold text: inside its border, and for the chat
    /// only the message area.
    fn selectable_area(&self, panel: PanelFocus) -> Option<Rect> {
        let area = match panel {
            PanelFocus::Chat => return self.areas.chat_text.or(self.areas.chat),
            PanelFocus::Swarm => self.areas.swarm,
            PanelFocus::Definitions => self.areas.definitions,
            PanelFocus::Records => self.areas.records,
            PanelFocus::Sandbox => self.areas.sandbox,
        }?;
        Some(Rect {
            x: area.x.saturating_add(1),
            y: area.y.saturating_add(1),
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        })
    }

    /// Text selected with the mouse, if any.
    #[must_use]
    pub const fn selection(&self) -> Option<&Selection> {
        self.selection.as_ref()
    }

    /// Drop the mouse selection.
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.selecting = false;
        self.copy_requested = false;
    }

    /// Whether a copy is waiting for the next frame to be drawn.
    #[must_use]
    pub const fn copy_pending(&self) -> bool {
        self.copy_requested
    }

    /// Ask for the current selection to be copied after the next frame.
    /// Returns `false` when nothing is selected.
    pub fn request_copy(&mut self) -> bool {
        self.copy_requested = self.selection.is_some_and(|s| !s.is_empty());
        self.copy_requested
    }

    /// The selected text in `buffer` (the frame just drawn), once per
    /// copy request.
    pub fn take_copy_text(&mut self, buffer: &Buffer) -> Option<String> {
        if !std::mem::take(&mut self.copy_requested) {
            return None;
        }
        let text = self.selection?.text(buffer);
        (!text.is_empty()).then_some(text)
    }

    /// Report the outcome of copying `chars` characters to the clipboard.
    pub fn notify_copied(&mut self, chars: usize, result: &std::io::Result<()>) {
        self.notification = Some(match result {
            Ok(()) => (
                format!(
                    "Copied {chars} character{}",
                    if chars == 1 { "" } else { "s" }
                ),
                NotificationType::Success,
            ),
            Err(e) => (format!("Copy failed: {e}"), NotificationType::Error),
        });
    }

    fn click_sandbox_row(&mut self, column: u16, row: u16) {
        let Some(rows) = self.areas.sandbox_rows else {
            return;
        };
        if !contains(Some(rows), column, row) {
            return;
        }
        let index = self.areas.sandbox_scroll + usize::from(row - rows.y);
        if index >= self.sandbox.rows().len() {
            return;
        }
        self.sandbox.select(index);
        self.sandbox.toggle_selected();
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

    /// Check if the Sandbox panel is visible.
    #[must_use]
    pub const fn sandbox_panel_visible(&self) -> bool {
        self.sandbox_panel_visible
    }

    /// File tree of the active agent's sandbox.
    #[must_use]
    pub const fn sandbox(&self) -> &SandboxTree {
        &self.sandbox
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

    /// Bash-style `[user@host dir]$ ` prompt shown before the input line.
    #[must_use]
    pub fn shell_prompt(&self) -> &str {
        &self.shell_prompt
    }

    #[cfg(test)]
    pub(crate) fn set_shell_prompt(&mut self, prompt: &str) {
        self.shell_prompt = prompt.to_string();
    }

    /// Whether the input should be highlighted as a shell command, as of the
    /// last [`Self::sync_input_kind`].
    #[must_use]
    pub const fn input_is_command(&self) -> bool {
        self.input_is_command
    }

    /// Re-check whether the current input is a shell command. Looking a
    /// program up on `PATH` is cached, so this is cheap to call per frame.
    ///
    /// Input only becomes a command once it has sat unchanged for
    /// [`COMMAND_HIGHLIGHT_DELAY`] (or starts with `!`), and stops being one
    /// as soon as it no longer parses as one.
    pub fn sync_input_kind(&mut self) {
        if matches!(self.state, AppState::LoginEmail | AppState::LoginPassword) {
            self.input_is_command = false;
            return;
        }
        if self.input != self.input_kind_text {
            self.input_kind_text.clone_from(&self.input);
            self.input_edited_at = Instant::now();
        }
        let lookups = &mut self.path_lookups;
        let is_command = shell_input::parse_with(&self.input, |program| {
            *lookups
                .entry(program.to_string())
                .or_insert_with(|| shell_input::on_path(program))
        })
        .is_some();
        let forced = self.input.trim_start().starts_with('!');
        let settled = self.input_edited_at.elapsed() >= COMMAND_HIGHLIGHT_DELAY;
        self.input_is_command = is_command && (self.input_is_command || forced || settled);
    }

    /// Record the width of the chat text area drawn this frame.
    pub fn set_chat_width(&mut self, width: u16) {
        self.chat_width = width;
    }

    /// Terminal width for a command, matching where its output is drawn.
    pub(super) fn command_columns(&self) -> u16 {
        if self.chat_width == 0 {
            return DEFAULT_COMMAND_COLUMNS;
        }
        self.chat_width.max(MIN_COMMAND_COLUMNS)
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
