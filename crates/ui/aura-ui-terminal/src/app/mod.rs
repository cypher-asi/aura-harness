//! Application state machine for the terminal UI.
//!
//! Manages the overall application state, conversation history,
//! panels (Chat, Record and Sandbox), and coordinates between input handling and rendering.

mod agent_views;
mod archive;
mod chat_tabs;
mod commands;
mod context_menu;
mod definitions;
pub mod file_view;
pub mod flow;
pub mod folder_picker;
pub(crate) use aura_ui::format;
pub mod graph;
pub mod hud;
pub mod image_preview;
pub mod images;
mod input_editor;
mod keys;
mod line_edit;
pub mod mentions;
mod panes;
pub mod plans;
pub mod tasks;
mod processes;
mod prose;
pub use aura_ui::sandbox;
mod selection;
pub(crate) mod shell_input;
mod shell_prompt;
mod slash_menu;
mod smooth;
pub mod tab_bar;
mod tool_view;
pub mod trace_feed;
mod trace_json;
mod turn;
pub mod vim;

#[cfg(test)]
mod detail_tests;
#[cfg(test)]
mod tests;

use crate::{
    alerts::AlertSettings,
    components::{Activity, Message},
    events::{
        AgentDefinitionSummary, AgentSummary, ModelOptionView, PlanView, ProcessSummary,
        RecordSummary, UiCommand, UiEvent,
    },
    input::InputHistory,
    renderer::{ChatLineCache, GroupState},
};
pub use chat_tabs::ChatTab;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
pub use definitions::AgentsTab;
use file_view::FileView;
use folder_picker::FolderPicker;
use hud::Hud;
pub use panes::{Pane, PaneChrome, PaneSplits, PaneTab};
pub use processes::SidePanelTab;
pub use prose::{ProseMeasure, ProseRow};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use sandbox::{SandboxEntryKind, SandboxTree};
pub use selection::Selection;
pub use smooth::ChatScroll;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Maximum number of messages to keep in history. Every tool call, thinking
/// block and policy line counts as one.
const MAX_MESSAGES: usize = 2000;

/// Terminal width given to commands run before the chat is first drawn.
const DEFAULT_COMMAND_COLUMNS: u16 = 80;

/// Narrowest terminal a command is given.
const MIN_COMMAND_COLUMNS: u16 = 20;

/// How long "Copied…" stays centered on the AURA CLI line.
const COPY_NOTICE_TTL: Duration = Duration::from_secs(2);

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
    /// Plan link flow: waiting for the pasted sign-in code
    LinkCode,
    /// Waiting for a project name (create or rename)
    ProjectName,
    /// Model selector overlay is open
    SelectingModel,
    /// Folder browser for a new project is open
    SelectingFolder,
}

/// Why the project-name prompt is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ProjectNamePurpose {
    /// Name a project that does not exist yet, kept in this folder
    Create(PathBuf),
    /// Rename the project that contains the active agent
    Rename,
}

/// Pending approval request.
#[derive(Debug, Clone)]
pub struct PendingApproval {
    /// Approval request ID
    pub id: String,
    /// Tool name
    pub tool: String,
    /// Description of the action
    pub description: String,
    /// What "always allow" would allow; empty when it isn't offered
    pub always: String,
}

/// Main application struct managing UI state.
#[allow(clippy::struct_excessive_bools)]
pub struct App {
    /// Current application state
    pub(super) state: AppState,
    /// What submitting the project-name prompt will do
    pub(super) project_name_purpose: Option<ProjectNamePurpose>,
    /// Conversation messages
    pub(super) messages: VecDeque<Message>,
    /// Current input text
    pub(super) input: String,
    /// Input history for navigation
    pub(super) input_history: InputHistory,
    /// Cursor position in input
    pub(super) cursor_pos: usize,
    /// Current status message
    pub(super) status: String,
    /// Calls waiting for an answer, oldest first; the front one is shown.
    /// Calls of one turn ask together, so one prompt must not replace
    /// another.
    pub(super) pending_approvals: VecDeque<PendingApproval>,
    /// Chat lines below the bottom of the chat view (0 = following the end)
    pub(super) scroll_offset: usize,
    /// Rendered lines of each message, reused between frames
    pub(super) chat_lines: ChatLineCache,
    /// Rendered lines of each shell command, for the terminal pane
    pub(super) terminal_lines: ChatLineCache,
    /// Total chat lines in the last frame
    pub(super) chat_line_count: usize,
    /// Chat rows of the last frame that expand or collapse a group of
    /// steps or one step when clicked
    pub(super) toggle_rows: Vec<ToggleRow>,
    /// First chat line shown in the last frame
    pub(super) chat_first_line: usize,
    /// Whether new activity blocks start expanded (`Ctrl+O` toggles)
    pub(super) activity_expanded: bool,
    /// Groups of steps the user opened (`true`) or closed, by the id of
    /// their first message
    pub(super) group_open: HashMap<u64, bool>,
    /// Whether verbose mode is enabled
    pub(super) verbose: bool,
    /// Event sender (for sending events to kernel)
    pub(super) event_tx: Option<mpsc::Sender<UiEvent>>,
    /// Command receiver (for receiving commands from kernel)
    pub(super) command_rx: Option<mpsc::Receiver<UiCommand>>,
    /// Current streaming message (being built)
    pub(super) streaming_content: String,
    /// Notification message (ephemeral)
    pub(super) notification: Option<(String, NotificationType)>,
    /// Copy confirmation drawn in the middle of the AURA CLI line until `Instant`.
    pub(super) copy_notice: Option<(String, NotificationType, Instant)>,
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
    /// Whether a tool may have changed files since the tree was read; the
    /// next tick reads it again, once however many tools finished
    pub(super) sandbox_stale: bool,
    /// Animation frame counter for spinners
    pub(super) animation_frame: usize,
    /// Kernel records list
    pub(super) records: VecDeque<RecordSummary>,
    /// Selected record index in the list
    pub(super) selected_record: usize,
    /// Scroll offset for records list
    pub(super) records_scroll: usize,
    /// Record or file shown over the chat panel
    pub(super) detail: Option<DetailView>,
    /// First line of the detail view shown
    pub(super) detail_scroll: usize,
    /// Source lines on screen in the detail view in the last frame
    pub(super) detail_shown: usize,
    /// List of agents in the swarm
    pub(super) agents: Vec<AgentSummary>,
    /// Highlighted row of the projects list (headers, agents, "+ New project")
    pub(super) selected_agent: usize,
    /// Root-agent ids whose project row is folded shut
    pub(super) collapsed_projects: HashSet<String>,
    /// Currently active agent ID
    pub(super) active_agent_id: String,
    /// Chats of the agents not on screen, by agent ID
    pub(super) views: HashMap<String, agent_views::AgentView>,
    /// API URL for the swarm
    pub(super) api_url: Option<String>,
    /// Whether the API is currently active
    pub(super) api_active: bool,
    /// Stored email during login flow (between email and password steps)
    pub(super) login_email: String,
    /// Models offered by the selector
    pub(super) models: Vec<ModelOptionView>,
    /// Id of the selected model (`gate` when Gate picks it)
    pub(super) current_model: String,
    /// The model and route Gate sends the next turn to
    pub(super) model_route: Option<String>,
    /// Logged-in user's display name (`None` = guest)
    pub(super) user: Option<String>,
    /// Labels of the usable linked plans
    pub(super) plans: Vec<String>,
    /// The active agent's current plan
    pub(super) active_plan: Option<PlanView>,
    /// Highlighted row in the model selector
    pub(super) model_cursor: usize,
    /// Cloud model to select once the in-chat login completes
    pub(super) pending_model: Option<String>,
    /// First visible row of the agents list
    pub(super) agents_scroll: usize,
    /// The wheel moved the agents list away from the highlight; it stays
    /// there until a key or click moves the highlight again
    pub(super) agents_scroll_detached: bool,
    /// First visible row of the sandbox tree
    pub(super) sandbox_scroll: usize,
    /// Screen areas from the last frame, for mouse hit-testing
    pub(super) areas: PanelAreas,
    /// The chat's prose rows from the last frame
    prose_rows: Vec<ProseRow>,
    /// How wide prose is in the front end's proportional face, if it has one
    prose_measure: Option<ProseMeasure>,
    /// The chat as last drawn, for a front end that scrolls it by pixels
    smooth: smooth::SmoothScroll,
    /// Whether the front end draws the panels' title bars itself
    host_chrome: bool,
    /// Whether the front end draws the GRAPH tab's canvas itself
    host_canvas: bool,
    /// What each panel's title bar showed in the last frame, for a front
    /// end that draws them
    pane_chrome: Vec<PaneChrome>,
    /// Where the front end put the dividers between panels
    pane_splits: PaneSplits,
    /// Clickable item under the mouse pointer
    hover: Option<HoverTarget>,
    /// When the in-flight turn started (`None` unless processing)
    pub(super) processing_since: Option<Instant>,
    /// `[user@host dir]$ ` shown before the input line at the terminal prompt
    pub(super) shell_prompt: String,
    /// Width of the chat text area in the last frame (0 before the first)
    pub(super) chat_width: u16,
    /// Whether the chat panel's TERMINAL tab is open (`$` opens it): the
    /// panel shows the session's shell commands, and Enter runs the line
    /// as one
    pub(super) terminal_mode: bool,
    /// Commands run at the terminal prompt, recalled there instead of chat
    /// history
    pub(super) command_history: InputHistory,
    /// Highlighted row in the slash menu
    pub(super) slash_cursor: usize,
    /// Whether Esc closed the slash menu (until the input is edited)
    pub(super) slash_dismissed: bool,
    /// `@` file picker shown while an `@path` word is typed
    pub(super) mentions: mentions::MentionMenu,
    /// Images pasted into the input, each under an `[image N]` chip
    pub(super) images: Vec<images::PendingImage>,
    /// Whether Ctrl+V asked for the clipboard before the next frame
    pub(super) paste_requested: bool,
    /// Images of messages sent to a busy agent, by message text, until
    /// the chat shows the message
    pub(super) queued_images: VecDeque<(String, Vec<std::sync::Arc<image_preview::ImagePreview>>)>,
    /// Image thumbnails in the chat in the last frame
    pub(super) image_hits: Vec<ImageHit>,
    /// Thumbnails of the images waiting in the input, in the last frame
    pub(super) pending_image_areas: Vec<(Rect, usize)>,
    /// Text selected with the mouse, in screen cells
    pub(super) selection: Option<Selection>,
    /// Whether the left button is still held for `selection`
    pub(super) selecting: bool,
    /// Whether the selection should be copied after the next frame
    pub(super) copy_requested: bool,
    /// Which list the agents panel shows
    pub(super) agents_tab: AgentsTab,
    /// Permanent agents
    pub(super) definitions: Vec<AgentDefinitionSummary>,
    /// Highlighted row in the roster
    pub(super) selected_definition: usize,
    /// First visible row of the roster
    pub(super) definitions_scroll: usize,
    /// File to open in the editor before the next frame
    pub(super) editor_request: Option<PathBuf>,
    /// Temp file holding the input while Ctrl+G has it in the editor
    pub(super) input_editor_file: Option<PathBuf>,
    /// Permanent agent waiting for `y` to confirm its deletion
    pub(super) pending_delete: Option<String>,
    /// Agent whose project waits for `y` to confirm removing it
    pub(super) pending_remove_project: Option<String>,
    /// Archived agents, each followed by the agents under it
    pub(super) archived: Vec<AgentSummary>,
    /// Highlighted row of the Archive
    pub(super) selected_archived: usize,
    /// First visible row of the Archive
    pub(super) archived_scroll: usize,
    /// Folder browser for a new project, while it is open
    pub(super) folder_picker: Option<FolderPicker>,
    /// Menu a right click opened on the projects list
    pub(super) context_menu: Option<context_menu::ContextMenu>,
    /// Usage, todos and jobs of the agent on screen
    pub(super) hud: Hud,
    /// Whether the full todo list is shown above the input
    pub(super) todos_open: bool,
    /// When to get the user's attention
    pub(super) alert_settings: AlertSettings,
    /// Messages for the user that the terminal should ring the bell for
    pub(super) alerts: Vec<String>,
    /// Which view the panel below the Record panel shows
    pub(super) side_tab: SidePanelTab,
    /// Rows of the Processes tab
    pub(super) processes: Vec<ProcessSummary>,
    /// Highlighted row of the Processes tab
    pub(super) selected_process: usize,
    /// First visible row of the Processes tab
    pub(super) processes_scroll: usize,
    /// Process waiting for `y` to confirm killing it
    pub(super) pending_kill: Option<String>,
    /// Run graph shown by `/graph`
    pub(super) graph: graph::GraphState,
    pub(super) trace_feed: trace_feed::TraceFeed,
    /// Plans of the PLANS tab
    pub(super) plan_list: plans::PlanList,
    /// Tasks of the TASKS tab
    pub(super) task_list: tasks::TaskList,
    /// Node to select once the requested graph loads.
    pub(super) pending_graph_node: Option<String>,
    /// Flow between agents shown by the chat panel's GRAPH tab
    pub(super) flow: flow::FlowState,
}

/// Screen areas of the content panels from the last rendered frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct PanelAreas {
    /// Agents panel (including border)
    pub swarm: Option<Rect>,
    /// Rows of the agents list inside the panel
    pub agent_rows: Option<Rect>,
    /// "+ New project", pinned to the bottom of the agents panel
    pub new_project_row: Option<Rect>,
    /// Header bar of the agents panel
    pub agent_tabs: Option<Rect>,
    /// Rows of the roster inside the agents panel
    pub definition_rows: Option<Rect>,
    /// "+ New agent", pinned to the bottom of the agents panel
    pub new_agent_row: Option<Rect>,
    /// Rows of the Archive inside the agents panel
    pub archived_rows: Option<Rect>,
    /// Chat panel (including border)
    pub chat: Option<Rect>,
    /// Header bar of the chat panel
    pub chat_tabs: Option<Rect>,
    /// Message text area inside the chat panel
    pub chat_text: Option<Rect>,
    /// Record panel (including border)
    pub records: Option<Rect>,
    /// Rows of the record list inside the panel
    pub record_rows: Option<Rect>,
    /// Sandbox panel (including border)
    pub sandbox: Option<Rect>,
    /// Rows of the sandbox tree inside the panel
    pub sandbox_rows: Option<Rect>,
    /// Tree row shown at the top of `sandbox_rows`
    pub sandbox_scroll: usize,
    /// Header bar of the panel below the Record panel
    pub side_tabs: Option<Rect>,
    /// Rows of the Processes tab inside the panel
    pub process_rows: Option<Rect>,
    /// Process row shown at the top of `process_rows`
    pub processes_scroll: usize,
    /// Rows of the Trace tab inside the panel
    pub trace_rows: Option<Rect>,
    /// Feed row shown at the top of `trace_rows`
    pub trace_scroll: usize,
    /// Rows of the Plans tab inside the panel
    pub plan_rows: Option<Rect>,
    /// Plans row shown at the top of `plan_rows`
    pub plans_scroll: usize,
    /// Rows of the Tasks tab inside the panel
    pub task_rows: Option<Rect>,
    /// Tasks row shown at the top of `task_rows`
    pub tasks_scroll: usize,
}

/// What is shown over the chat panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailView {
    /// The record highlighted in the Record panel.
    Record,
    /// A file from the sandbox.
    File(Box<FileView>),
    /// A process from the Processes tab, refreshed while it stays listed.
    Process(Box<ProcessSummary>),
    /// The run graph (`/graph`).
    Graph,
    /// A pasted image, as large as the panel allows.
    Image(Box<image_preview::ImageView>),
}

/// Clickable item under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoverTarget {
    /// A label in the agents tab bar.
    AgentTab(AgentsTab),
    /// A label in the chat panel's tab bar.
    ChatTab(ChatTab),
    /// A row in the projects list, including "+ New project".
    ProjectRow(usize),
    /// The "+" at the right end of a project's row in the projects list.
    AddToProject(usize),
    /// A row in the roster, including "+ New agent".
    DefinitionRow(usize),
    /// A row in the Archive.
    ArchivedRow(usize),
    /// A row in the sandbox tree.
    SandboxRow(usize),
    /// A row in the Record panel.
    RecordRow(usize),
    /// A label in the side panel's tab bar.
    SideTab(SidePanelTab),
    /// A row in the Processes tab.
    ProcessRow(usize),
    /// A row in the Trace tab.
    TraceRow(usize),
    /// A row in the Plans tab.
    PlanRow(usize),
    /// A row in the Tasks tab.
    TaskRow(usize),
    /// A chat row that expands or collapses agent activity.
    Activity(ToggleTarget),
    /// An image thumbnail in the chat: message id, which of its images.
    ChatImage(u64, usize),
    /// A thumbnail above the input, by its `[image N]` label.
    PendingImage(usize),
}

/// What clicking an activity row in the chat expands or collapses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ToggleTarget {
    /// A group of steps, by the id of its first message
    Group(u64),
    /// One step, by message id
    Step(u64),
    /// The kernel records of a turn, by when its prompt was sent (Unix ms)
    TurnRecords(u64),
}

/// A thumbnail in the chat that opens its image when clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImageHit {
    /// Chat line of the thumbnail's first row
    pub line: usize,
    /// Rows the thumbnail takes
    pub rows: usize,
    /// First column, counted from the chat text's left edge
    pub x: u16,
    /// Columns the thumbnail takes
    pub width: u16,
    /// Id of the message holding the image
    pub message: u64,
    /// Which of the message's images
    pub index: usize,
}

/// A chat row that expands or collapses `target` when clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToggleRow {
    /// Chat line of the row
    pub line: usize,
    /// What the row toggles
    pub target: ToggleTarget,
    /// Whether `target` is shown open
    pub open: bool,
}

/// The tab of the header bar drawn at `bar` under a screen cell, laid out
/// by `layout` for the bar's width.
fn tab_at<T: Copy + PartialEq>(
    bar: Option<Rect>,
    column: u16,
    row: u16,
    layout: impl Fn(u16) -> tab_bar::TabLayout<T>,
) -> Option<T> {
    let bar = bar.filter(|&bar| contains(Some(bar), column, row))?;
    layout(bar.width).tab_at(column - bar.x)
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

/// Columns of the " + " at the right end of a project's row, which adds an
/// agent to the project when clicked.
pub(crate) const PROJECT_ADD_WIDTH: u16 = 3;

/// One row of the projects list. A project is its own row; its first
/// agent, and any agents that agent spawned, hang underneath while the
/// project is expanded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectListRow {
    /// Foldable header. `root` is the index of the project's first agent.
    Project { root: usize, expanded: bool },
    /// An agent under a project.
    Agent { index: usize },
    /// The "+ New project" row.
    NewProject,
    /// Blank row between two projects. Never selected.
    Gap,
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
            project_name_purpose: None,
            messages: VecDeque::new(),
            input: String::new(),
            input_history: InputHistory::new(),
            cursor_pos: 0,
            status: "Ready".to_string(),
            pending_approvals: VecDeque::new(),
            scroll_offset: 0,
            chat_lines: ChatLineCache::default(),
            terminal_lines: ChatLineCache::terminal(),
            chat_line_count: 0,
            toggle_rows: Vec::new(),
            chat_first_line: 0,
            activity_expanded: false,
            group_open: HashMap::new(),
            verbose: false,
            event_tx: None,
            command_rx: None,
            streaming_content: String::new(),
            notification: None,
            copy_notice: None,
            focus: PanelFocus::default(),
            record_panel_visible: true,
            swarm_panel_visible: true,
            sandbox_panel_visible: true,
            sandbox: SandboxTree::default(),
            sandbox_stale: false,
            animation_frame: 0,
            records: VecDeque::new(),
            selected_record: 0,
            records_scroll: 0,
            detail: None,
            detail_scroll: 0,
            detail_shown: 1,
            agents: Vec::new(),
            selected_agent: 0,
            collapsed_projects: HashSet::new(),
            active_agent_id: String::new(),
            views: HashMap::new(),
            api_url: None,
            api_active: false,
            login_email: String::new(),
            models: Vec::new(),
            current_model: String::new(),
            model_route: None,
            user: None,
            plans: Vec::new(),
            active_plan: None,
            model_cursor: 0,
            pending_model: None,
            agents_scroll: 0,
            agents_scroll_detached: false,
            sandbox_scroll: 0,
            areas: PanelAreas::default(),
            prose_rows: Vec::new(),
            prose_measure: None,
            smooth: smooth::SmoothScroll::default(),
            host_chrome: false,
            host_canvas: false,
            pane_chrome: Vec::new(),
            pane_splits: PaneSplits::default(),
            hover: None,
            processing_since: None,
            shell_prompt: shell_prompt::detect(),
            chat_width: 0,
            terminal_mode: false,
            command_history: InputHistory::new(),
            slash_cursor: 0,
            slash_dismissed: false,
            mentions: mentions::MentionMenu::default(),
            images: Vec::new(),
            paste_requested: false,
            queued_images: VecDeque::new(),
            image_hits: Vec::new(),
            pending_image_areas: Vec::new(),
            selection: None,
            selecting: false,
            copy_requested: false,
            agents_tab: AgentsTab::default(),
            definitions: Vec::new(),
            selected_definition: 0,
            definitions_scroll: 0,
            editor_request: None,
            input_editor_file: None,
            pending_delete: None,
            pending_remove_project: None,
            archived: Vec::new(),
            selected_archived: 0,
            archived_scroll: 0,
            folder_picker: None,
            context_menu: None,
            hud: Hud::default(),
            todos_open: false,
            alert_settings: AlertSettings::default(),
            alerts: Vec::new(),
            side_tab: SidePanelTab::default(),
            processes: Vec::new(),
            selected_process: 0,
            processes_scroll: 0,
            pending_kill: None,
            graph: graph::GraphState::default(),
            trace_feed: trace_feed::TraceFeed::default(),
            plan_list: plans::PlanList::default(),
            task_list: tasks::TaskList::default(),
            pending_graph_node: None,
            flow: flow::FlowState::default(),
        }
    }

    /// Get the user's attention according to `settings`.
    #[must_use]
    pub const fn with_alert_settings(mut self, settings: AlertSettings) -> Self {
        self.alert_settings = settings;
        self
    }

    /// When the terminal gets the user's attention.
    #[must_use]
    pub const fn alert_settings(&self) -> &AlertSettings {
        &self.alert_settings
    }

    /// Messages to get the user's attention for since the last call.
    pub fn take_alerts(&mut self) -> Vec<String> {
        std::mem::take(&mut self.alerts)
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
        if let Some(run) = self
            .messages
            .back()
            .filter(|m| m.is_running_shell())
            .and_then(Message::shell_run)
        {
            let program = run.command.split_whitespace().next().unwrap_or_default();
            return Some(format!("running {program}"));
        }
        if let Some(Activity::Tool(tool)) = self
            .messages
            .iter()
            .rev()
            .filter_map(Message::activity_step)
            .find(|a| a.is_running())
        {
            return Some(format!("running {}", tool.name));
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

    /// Usage, todos and jobs of the agent on screen.
    #[must_use]
    pub const fn hud(&self) -> &Hud {
        &self.hud
    }

    /// Whether the full todo list is shown above the input.
    #[must_use]
    pub const fn todos_open(&self) -> bool {
        self.todos_open
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
        self.activity_expanded
    }

    /// Expand every activity block, or collapse them all if they were
    /// expanded. New blocks follow suit.
    pub fn toggle_all_activity(&mut self) {
        self.activity_expanded = !self.activity_expanded;
        let expanded = self.activity_expanded;
        self.group_open.clear();
        for message in &mut self.messages {
            message.set_expanded(expanded);
        }
    }

    /// Whether the conversation ends in a group of steps that the turn in
    /// flight may still add to. Its summary line spins in place of the
    /// working indicator.
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

    /// Record where activity rows were drawn: `rows` holds the chat line
    /// of each one, and `first_line` is the chat line at the top of the
    /// view.
    pub(crate) fn set_chat_layout(&mut self, rows: Vec<ToggleRow>, first_line: usize) {
        self.toggle_rows = rows;
        self.chat_first_line = first_line;
    }

    /// The activity row at a screen cell, using the layout of the last frame.
    fn toggle_row_at(&self, column: u16, row: u16) -> Option<ToggleRow> {
        let area = self
            .areas
            .chat_text
            .filter(|area| contains(Some(*area), column, row))?;
        let line = self.chat_first_line + usize::from(row - area.y);
        self.toggle_rows.iter().find(|r| r.line == line).copied()
    }

    /// Open or close the group of steps, or the step, at screen `row`.
    /// Returns whether anything was toggled.
    fn toggle_activity_at(&mut self, column: u16, row: u16) -> bool {
        let Some(toggle) = self.toggle_row_at(column, row) else {
            return false;
        };
        match toggle.target {
            ToggleTarget::Group(id) => {
                self.group_open.insert(id, !toggle.open);
            }
            ToggleTarget::Step(id) => {
                if self.open_tool_step(id) {
                    return true;
                }
                let Some(message) = self.messages.iter_mut().find(|m| m.version().0 == id) else {
                    return false;
                };
                let expanded = !message.is_expanded();
                message.set_expanded(expanded);
            }
            ToggleTarget::TurnRecords(started_ms) => self.show_turn_records(started_ms),
        }
        true
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

    /// Messages along with the cache of the lines the open pane draws them
    /// as.
    pub(crate) fn chat_line_cache(
        &mut self,
    ) -> (&VecDeque<Message>, &mut ChatLineCache, GroupState<'_>) {
        let groups = GroupState {
            open: &self.group_open,
            expand_all: self.activity_expanded,
            turn_live: self.state == AppState::Processing,
        };
        let cache = if self.terminal_mode {
            self.terminal_lines.set_prompt(&self.shell_prompt);
            &mut self.terminal_lines
        } else {
            &mut self.chat_lines
        };
        (&self.messages, cache, groups)
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
        !self
            .messages
            .iter()
            .any(|m| m.shell_run().is_some() == self.terminal_mode)
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
            PanelFocus::Chat if self.flow.is_open() => {
                self.scroll_flow(MouseEventKind::ScrollUp, KeyModifiers::NONE, lines);
            }
            PanelFocus::Chat => {
                self.scroll_offset = self.scroll_offset.saturating_add(lines);
            }
            PanelFocus::Records => {
                self.selected_record = self.selected_record.saturating_sub(lines);
            }
            PanelFocus::Swarm => match self.agents_tab {
                AgentsTab::Projects => {
                    self.agents_scroll = self.agents_scroll.saturating_sub(lines);
                    self.agents_scroll_detached = true;
                }
                AgentsTab::Roster => {
                    self.select_definition_row(self.selected_definition.saturating_sub(lines));
                }
                AgentsTab::Archive => {
                    self.select_archived_row(self.selected_archived.saturating_sub(lines));
                }
            },
            PanelFocus::Sandbox => {
                let delta = -isize::try_from(lines).unwrap_or(isize::MAX);
                match self.side_tab {
                    SidePanelTab::Sandbox => self.sandbox.move_by(delta),
                    SidePanelTab::Processes => self.scroll_processes(delta),
                    SidePanelTab::Trace => self.trace_feed.move_by(delta),
                    SidePanelTab::Plans => self.plan_list.move_by(delta),
                    SidePanelTab::Tasks => self.task_list.move_by(delta),
                }
            }
        }
    }

    fn scroll_panel_down(&mut self, panel: PanelFocus, lines: usize) {
        match panel {
            PanelFocus::Chat if self.flow.is_open() => {
                self.scroll_flow(MouseEventKind::ScrollDown, KeyModifiers::NONE, lines);
            }
            PanelFocus::Chat => {
                self.scroll_offset = self.scroll_offset.saturating_sub(lines);
            }
            PanelFocus::Records => {
                if !self.records.is_empty() {
                    self.selected_record =
                        (self.selected_record + lines).min(self.records.len().saturating_sub(1));
                }
            }
            PanelFocus::Swarm => match self.agents_tab {
                AgentsTab::Projects => {
                    let visible = self
                        .areas
                        .agent_rows
                        .map_or(1, |area| usize::from(area.height));
                    let max = self.listed_project_rows().saturating_sub(visible);
                    self.agents_scroll = (self.agents_scroll + lines).min(max);
                    self.agents_scroll_detached = true;
                }
                AgentsTab::Roster => {
                    self.select_definition_row(self.selected_definition.saturating_add(lines));
                }
                AgentsTab::Archive => {
                    self.select_archived_row(self.selected_archived.saturating_add(lines));
                }
            },
            PanelFocus::Sandbox => {
                let delta = isize::try_from(lines).unwrap_or(isize::MAX);
                match self.side_tab {
                    SidePanelTab::Sandbox => self.sandbox.move_by(delta),
                    SidePanelTab::Processes => self.scroll_processes(delta),
                    SidePanelTab::Trace => self.trace_feed.move_by(delta),
                    SidePanelTab::Plans => self.plan_list.move_by(delta),
                    SidePanelTab::Tasks => self.task_list.move_by(delta),
                }
            }
        }
    }

    /// Highlight projects-list row `row`. Landing on an agent switches to it.
    /// Only agents and "+ New project" take the highlight: project headers
    /// and gaps are stepped over in the direction of travel, or the other
    /// way at the end of the list.
    pub(super) fn select_agent_row(&mut self, row: usize) {
        self.agents_scroll_detached = false;
        let rows = self.project_rows();
        if rows.is_empty() {
            self.selected_agent = 0;
            return;
        }
        let target = row.min(rows.len() - 1);
        let takes_highlight = |&row: &usize| {
            matches!(
                rows[row],
                ProjectListRow::Agent { .. } | ProjectListRow::NewProject
            )
        };
        let found = if target < self.selected_agent {
            (0..=target)
                .rev()
                .find(takes_highlight)
                .or_else(|| (target..rows.len()).find(takes_highlight))
        } else {
            (target..rows.len())
                .find(takes_highlight)
                .or_else(|| (0..=target).rev().find(takes_highlight))
        };
        let Some(row) = found else {
            return;
        };
        self.selected_agent = row;
        if let Some(ProjectListRow::Agent { index }) = rows.get(self.selected_agent) {
            let agent_id = self.agents[*index].id.clone();
            self.request_agent_switch(agent_id);
        }
    }

    /// Act on the highlighted projects-list row: fold a project, switch to
    /// an agent, or create one from "+ New project".
    pub(super) fn activate_agent_row(&mut self) {
        self.agents_scroll_detached = false;
        match self.project_rows().get(self.selected_agent).copied() {
            Some(ProjectListRow::Project { root, .. }) => self.toggle_project(root),
            Some(ProjectListRow::Agent { index }) => {
                let agent_id = self.agents[index].id.clone();
                self.request_agent_switch(agent_id);
            }
            Some(ProjectListRow::NewProject) | None => self.prompt_new_project(),
            Some(ProjectListRow::Gap) => {}
        }
    }

    /// Highlight the last agent, opening its project if it was folded.
    pub(super) fn select_last_agent_row(&mut self) {
        let Some(index) = self.agents.len().checked_sub(1) else {
            self.select_agent_row(0);
            return;
        };
        self.reveal_agent(index);
        let row = self.row_of_agent(index).unwrap_or(0);
        self.select_agent_row(row);
    }

    /// Ask for a folder and then a name, then create a project with a
    /// default agent under it. Browsing starts in the launch directory.
    pub(super) fn prompt_new_project(&mut self) {
        let start = std::env::current_dir()
            .ok()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        match FolderPicker::open(start) {
            Ok(picker) => {
                self.folder_picker = Some(picker);
                self.state = AppState::SelectingFolder;
                self.status = "Project folder".to_string();
            }
            Err(e) => {
                self.notification = Some((
                    format!("Cannot browse folders: {e}"),
                    NotificationType::Error,
                ));
            }
        }
    }

    /// Close the folder browser and ask for the new project's name,
    /// starting from the folder's own name.
    pub(super) fn choose_project_folder(&mut self, folder: PathBuf) {
        self.folder_picker = None;
        let name = folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.project_name_purpose = Some(ProjectNamePurpose::Create(folder));
        self.begin_project_name();
        self.cursor_pos = name.len();
        self.input = name;
    }

    /// Close the folder browser without creating a project.
    pub(super) fn cancel_folder_picker(&mut self) {
        self.folder_picker = None;
        self.state = AppState::Idle;
        self.status = "Ready".to_string();
        self.notification = Some(("Cancelled".to_string(), NotificationType::Warning));
    }

    /// Folder browser for a new project, while it is open.
    #[must_use]
    pub const fn folder_picker(&self) -> Option<&FolderPicker> {
        self.folder_picker.as_ref()
    }

    /// Keep the folder browser's highlight inside a list `visible_rows`
    /// tall, returning the first row to show.
    pub fn sync_folder_picker_scroll(&mut self, visible_rows: usize) -> usize {
        self.folder_picker
            .as_mut()
            .map_or(0, |picker| picker.sync_scroll(visible_rows))
    }

    /// Ask for a new name for the project that contains the active agent.
    pub(super) fn prompt_rename_project(&mut self) {
        self.project_name_purpose = Some(ProjectNamePurpose::Rename);
        self.begin_project_name();
    }

    fn begin_project_name(&mut self) {
        self.state = AppState::ProjectName;
        self.input.clear();
        self.cursor_pos = 0;
        self.status = "Project name".to_string();
    }

    /// Finish the project-name prompt. An empty name leaves the prompt open.
    pub(super) fn submit_project_name(&mut self, name: &str) {
        let name = name.trim().to_string();
        if name.is_empty() {
            self.notification = Some((
                "Project name cannot be empty".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        let purpose = self.project_name_purpose.take();
        self.state = AppState::Idle;
        self.status = "Ready".to_string();
        let event = match purpose {
            Some(ProjectNamePurpose::Create(folder)) => UiEvent::CreateProject { name, folder },
            Some(ProjectNamePurpose::Rename) | None => UiEvent::RenameProject(name),
        };
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(event);
        }
    }

    /// Fold or open the project under the highlight. An agent row folds the
    /// project that contains it.
    pub(super) fn toggle_selected_project(&mut self) {
        self.agents_scroll_detached = false;
        let root = match self.project_rows().get(self.selected_agent).copied() {
            Some(ProjectListRow::Project { root, .. }) => Some(root),
            Some(ProjectListRow::Agent { index }) => self.project_root(index),
            Some(ProjectListRow::NewProject | ProjectListRow::Gap) | None => None,
        };
        if let Some(root) = root {
            self.toggle_project(root);
        }
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
            .unwrap_or(0);
        let last = self.agents.len() - 1;
        let target = current.saturating_add_signed(delta).min(last);
        self.reveal_agent(target);
        let row = self.row_of_agent(target).unwrap_or(0);
        self.select_agent_row(row);
    }

    /// First visible line of the agents list.
    #[must_use]
    pub const fn agents_scroll(&self) -> usize {
        self.agents_scroll
    }

    /// Keep the highlighted projects-list row inside a list `visible_rows`
    /// tall. "+ New project" is pinned below the list, so it never scrolls.
    /// After the wheel scrolled the list, it stays put until the highlight
    /// moves again.
    pub fn sync_agents_scroll(&mut self, visible_rows: usize) {
        let total = self.listed_project_rows();
        let visible = visible_rows.max(1);
        self.agents_scroll = if self.agents_scroll_detached {
            self.agents_scroll.min(total.saturating_sub(visible))
        } else {
            follow_selection(self.agents_scroll, self.selected_agent, visible, total)
        };
    }

    /// Projects-list rows that scroll: all but the pinned "+ New project".
    fn listed_project_rows(&self) -> usize {
        self.project_rows().len().saturating_sub(1)
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

    /// Record where the record list rows were drawn this frame.
    pub fn set_record_rows_area(&mut self, area: Rect) {
        self.areas.record_rows = Some(area);
    }

    /// Record where "+ New project" was drawn this frame.
    pub fn set_new_project_area(&mut self, area: Rect) {
        self.areas.new_project_row = Some(area);
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

    /// Clickable item under the pointer, if it is over one.
    #[must_use]
    pub(crate) const fn hover(&self) -> Option<HoverTarget> {
        self.hover
    }

    /// Handle a mouse event: the wheel scrolls the panel under the pointer,
    /// a left click focuses it, clicking an agent row switches to it,
    /// clicking a record shows it over the chat panel, and clicking a
    /// sandbox row selects it and opens or closes a directory, or shows a
    /// file over the chat panel.
    /// Dragging with the left button selects text inside the panel the
    /// drag started in and copies it when the button is released.
    /// Moving over a clickable item highlights it until the pointer leaves.
    ///
    /// Returns whether the screen may look different: false for a move or
    /// a drag that left the highlight and the selection as they were.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        let pointing = matches!(mouse.kind, MouseEventKind::Moved | MouseEventKind::Drag(_));
        let menu = |app: &Self| app.context_menu.as_ref().map(|menu| menu.selected);
        let before = (self.hover, self.selection, menu(self));
        self.mouse(mouse);
        !pointing || before != (self.hover, self.selection, menu(self))
    }

    fn mouse(&mut self, mouse: MouseEvent) {
        if self.state != AppState::Idle && self.state != AppState::Processing {
            self.hover = None;
            self.context_menu = None;
            return;
        }
        if self.context_menu_mouse(mouse.kind, mouse.column, mouse.row) {
            self.hover = None;
            return;
        }
        if mouse.kind == MouseEventKind::Down(MouseButton::Right)
            && self.panel_at(mouse.column, mouse.row) == Some(PanelFocus::Swarm)
            && self.open_context_menu(mouse.column, mouse.row)
        {
            self.clear_selection();
            self.hover = None;
            return;
        }
        let at = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                self.hover = None;
                if self.drag_flow(mouse.column, mouse.row) {
                    return;
                }
                if let Some(selection) = self.selection.as_mut().filter(|_| self.selecting) {
                    selection.extend_to(at);
                }
                return;
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.release_flow() {
                    return;
                }
                if std::mem::take(&mut self.selecting) {
                    if self.selection.is_some_and(|s| !s.is_empty()) {
                        self.copy_requested = true;
                    } else {
                        self.selection = None;
                        if self.detail.is_none() {
                            self.toggle_activity_at(mouse.column, mouse.row);
                        }
                    }
                }
                self.hover = self.hover_target(mouse.column, mouse.row);
                return;
            }
            MouseEventKind::Moved => {
                self.hover = self.hover_target(mouse.column, mouse.row);
                return;
            }
            _ => {}
        }
        let Some(panel) = self.panel_at(mouse.column, mouse.row) else {
            return;
        };
        let picks_a_row = panel == PanelFocus::Swarm && self.agents_tab != AgentsTab::Projects;
        let step = if picks_a_row { 1 } else { 3 };
        let over_detail = panel == PanelFocus::Chat && self.detail.is_some();
        match mouse.kind {
            MouseEventKind::ScrollUp if over_detail => {
                self.clear_selection();
                self.scroll_detail_up(step);
            }
            MouseEventKind::ScrollDown if over_detail => {
                self.clear_selection();
                self.scroll_detail_down(step);
            }
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight
                if panel == PanelFocus::Chat && self.flow.is_open() =>
            {
                self.clear_selection();
                self.scroll_flow(mouse.kind, mouse.modifiers, step);
            }
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
                if panel == PanelFocus::Chat && self.open_image_at(mouse.column, mouse.row) {
                    self.clear_selection();
                    self.hover = None;
                    return;
                }
                self.selection = self
                    .selectable_area(panel)
                    .map(|bounds| Selection::new(bounds, at));
                self.selecting = self.selection.is_some();
                match panel {
                    PanelFocus::Swarm => self.click_agents_panel(mouse.column, mouse.row),
                    PanelFocus::Sandbox => self.click_side_panel(mouse.column, mouse.row),
                    PanelFocus::Records => self.click_record_row(mouse.column, mouse.row),
                    PanelFocus::Chat => {
                        if self.click_chat_tabs(mouse.column, mouse.row) {
                            self.clear_selection();
                        } else if self.detail == Some(DetailView::Graph) {
                            self.click_graph(mouse.column, mouse.row);
                        } else if self.detail.is_none()
                            && self.flow.is_open()
                            && self.click_flow(mouse.column, mouse.row)
                        {
                            self.clear_selection();
                        }
                    }
                }
            }
            _ => {}
        }
        self.hover = self.hover_target(mouse.column, mouse.row);
    }

    /// Clickable item at a screen cell, using the areas from the last frame.
    fn hover_target(&self, column: u16, row: u16) -> Option<HoverTarget> {
        if contains(self.areas.agent_tabs, column, row) {
            return tab_at(self.areas.agent_tabs, column, row, AgentsTab::layout)
                .map(HoverTarget::AgentTab);
        }
        if let Some(target) = self.hovered_chat_tab(column, row) {
            return Some(target);
        }
        if self.agents_tab == AgentsTab::Projects {
            if let Some(index) = self.project_row_at(column, row) {
                if self.project_add_at(index, column).is_some() {
                    return Some(HoverTarget::AddToProject(index));
                }
                return Some(HoverTarget::ProjectRow(index));
            }
        }
        if self.agents_tab == AgentsTab::Roster {
            if let Some(index) = self.definition_row_at(column, row) {
                return Some(HoverTarget::DefinitionRow(index));
            }
        }
        if self.agents_tab == AgentsTab::Archive {
            if let Some(index) = self.archived_row_at(column, row) {
                return Some(HoverTarget::ArchivedRow(index));
            }
        }
        if let Some(index) = self.record_row_at(column, row) {
            return Some(HoverTarget::RecordRow(index));
        }
        if let Some(target) = self.hovered_side_panel(column, row) {
            return Some(target);
        }
        if let Some(target) = self.image_at(column, row) {
            return Some(target);
        }
        self.hovered_activity(column, row)
    }

    /// Projects-list row drawn at a screen cell: a listed project or agent,
    /// or the pinned "+ New project". Gaps are not rows to act on.
    fn project_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self.project_rows();
        let new_project = rows.len().checked_sub(1)?;
        if contains(self.areas.new_project_row, column, row) {
            return Some(new_project);
        }
        let list = self.areas.agent_rows?;
        if !contains(Some(list), column, row) {
            return None;
        }
        let index = self.agents_scroll + usize::from(row - list.y);
        matches!(
            rows[..new_project].get(index)?,
            ProjectListRow::Project { .. } | ProjectListRow::Agent { .. }
        )
        .then_some(index)
    }

    /// Root agent of the project whose "+" is at `column` on projects-list
    /// row `index`.
    fn project_add_at(&self, index: usize, column: u16) -> Option<usize> {
        let list = self.areas.agent_rows?;
        let ProjectListRow::Project { root, .. } = *self.project_rows().get(index)? else {
            return None;
        };
        (column + PROJECT_ADD_WIDTH >= list.x + list.width).then_some(root)
    }

    /// Unfold the project headed by agent `root` and ask for a new agent in
    /// it.
    pub(super) fn add_agent_to_project(&mut self, root: usize) {
        let Some(id) = self.agents.get(root).map(|agent| agent.id.clone()) else {
            return;
        };
        self.collapsed_projects.remove(&id);
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::AddAgentToProject(id));
        }
    }

    fn record_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self
            .areas
            .record_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = self.records_scroll + usize::from(row - rows.y);
        (index < self.records.len()).then_some(index)
    }

    /// Clicking a record highlights it and shows it over the chat panel.
    fn click_record_row(&mut self, column: u16, row: u16) {
        if let Some(index) = self.record_row_at(column, row) {
            self.selected_record = index;
            self.open_detail(DetailView::Record);
        }
    }

    fn hovered_activity(&self, column: u16, row: u16) -> Option<HoverTarget> {
        if self.detail.is_some() {
            return None;
        }
        self.toggle_row_at(column, row)
            .map(|toggle| HoverTarget::Activity(toggle.target))
    }

    /// Cells of `panel` that hold text: inside its border, and for the chat
    /// only the message area unless a record or file covers it.
    fn selectable_area(&self, panel: PanelFocus) -> Option<Rect> {
        let area = match panel {
            PanelFocus::Chat if self.detail.is_some() => self.areas.chat,
            PanelFocus::Chat => return self.areas.chat_text.or(self.areas.chat),
            PanelFocus::Swarm => self.areas.swarm,
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
    ///
    /// Shown centered on the AURA CLI line, then cleared after a couple of seconds.
    pub fn notify_copied(&mut self, chars: usize, result: &std::io::Result<()>) {
        let (message, kind) = match result {
            Ok(()) => (
                format!(
                    "Copied {chars} character{}",
                    if chars == 1 { "" } else { "s" }
                ),
                NotificationType::Success,
            ),
            Err(e) => (format!("Copy failed: {e}"), NotificationType::Error),
        };
        self.copy_notice = Some((message, kind, Instant::now() + COPY_NOTICE_TTL));
    }

    /// Copy confirmation currently shown on the AURA CLI line.
    #[must_use]
    pub fn copy_notice(&self) -> Option<(&str, NotificationType)> {
        self.copy_notice
            .as_ref()
            .map(|(message, kind, _)| (message.as_str(), *kind))
    }

    /// Clicking a project folds or opens it and leaves the highlight where
    /// it was; clicking an agent or "+ New project" highlights and acts on it.
    fn click_agent_row(&mut self, column: u16, row: u16) {
        let Some(index) = self.project_row_at(column, row) else {
            return;
        };
        if let Some(root) = self.project_add_at(index, column) {
            self.add_agent_to_project(root);
        } else if let Some(ProjectListRow::Project { root, .. }) = self.project_rows().get(index) {
            self.toggle_project(*root);
        } else {
            self.selected_agent = index;
            self.activate_agent_row();
        }
    }

    /// The approval prompt on screen: the oldest call still waiting.
    #[must_use]
    pub fn pending_approval(&self) -> Option<&PendingApproval> {
        self.pending_approvals.front()
    }

    /// How many calls of the chat on screen wait for an answer.
    #[must_use]
    pub fn pending_approval_count(&self) -> usize {
        self.pending_approvals.len()
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

    /// Visible projects-list rows. Every project contributes a header and,
    /// while expanded, its agents with the agents they spawned.
    /// A gap row separates each project from the one above it.
    #[must_use]
    pub(crate) fn project_rows(&self) -> Vec<ProjectListRow> {
        let mut rows = Vec::new();
        let mut index = 0;
        while index < self.agents.len() {
            if self.agents[index].heads_project() {
                let root = index;
                if !rows.is_empty() {
                    rows.push(ProjectListRow::Gap);
                }
                let end = self.project_end(root);
                let expanded = !self.collapsed_projects.contains(&self.agents[root].id);
                rows.push(ProjectListRow::Project { root, expanded });
                if expanded {
                    for agent_index in root..end {
                        rows.push(ProjectListRow::Agent { index: agent_index });
                    }
                }
                index = end;
            } else {
                rows.push(ProjectListRow::Agent { index });
                index += 1;
            }
        }
        rows.push(ProjectListRow::NewProject);
        rows
    }

    /// Index just past the agents that belong to the project rooted at `root`.
    fn project_end(&self, root: usize) -> usize {
        self.agents
            .iter()
            .skip(root + 1)
            .position(|agent| agent.heads_project())
            .map_or(self.agents.len(), |offset| root + 1 + offset)
    }

    /// Root agent of the project that contains `index`.
    fn project_root(&self, index: usize) -> Option<usize> {
        (0..=index).rev().find(|&i| self.agents[i].heads_project())
    }

    /// Open a folded project so `index` has a visible row.
    fn reveal_agent(&mut self, index: usize) {
        if let Some(root) = self.project_root(index) {
            self.collapsed_projects.remove(&self.agents[root].id);
        }
    }

    /// Visible row of agent `index`, when its project is expanded.
    fn row_of_agent(&self, index: usize) -> Option<usize> {
        self.project_rows().iter().position(
            |row| matches!(row, ProjectListRow::Agent { index: found } if *found == index),
        )
    }

    /// Fold or open the project rooted at `root`. Folding it parks the
    /// highlight on the header, where it is not drawn, when the highlight
    /// was inside that project; opening it again brings the highlight back
    /// to the agent shown in the chat if it is in there. Otherwise the
    /// highlight stays on the same row as rows above it come and go.
    fn toggle_project(&mut self, root: usize) {
        let Some(id) = self.agents.get(root).map(|agent| agent.id.clone()) else {
            return;
        };
        let span = root..self.project_end(root);
        let before = self.project_rows().get(self.selected_agent).copied();
        let selected_inside = before.is_some_and(|row| match row {
            ProjectListRow::Project { root: found, .. } => found == root,
            ProjectListRow::Agent { index } => span.contains(&index),
            ProjectListRow::NewProject | ProjectListRow::Gap => false,
        });
        let opened = self.collapsed_projects.remove(&id);
        if !opened {
            self.collapsed_projects.insert(id);
        }
        let rows = self.project_rows();
        let active_row = || {
            span.clone()
                .find(|&index| self.agents[index].id == self.active_agent_id)
                .and_then(|index| self.row_of_agent(index))
        };
        let kept = if selected_inside {
            (opened.then(active_row).flatten()).or_else(|| {
                rows.iter().position(
                    |row| matches!(row, ProjectListRow::Project { root: found, .. } if *found == root),
                )
            })
        } else {
            before.and_then(|before| rows.iter().position(|row| *row == before))
        };
        if let Some(pos) = kept {
            self.selected_agent = pos;
        }
        let len = self.project_rows().len();
        self.selected_agent = self.selected_agent.min(len.saturating_sub(1));
    }

    /// Move the highlight to the active agent, or to its project when folded.
    fn select_visible_active(&mut self) {
        if let Some(index) = self
            .agents
            .iter()
            .position(|agent| agent.id == self.active_agent_id)
        {
            if let Some(row) = self.row_of_agent(index) {
                self.selected_agent = row;
                return;
            }
            if let Some(root) = self.project_root(index) {
                if let Some(row) = self.project_rows().iter().position(|row| {
                    matches!(row, ProjectListRow::Project { root: found, .. } if *found == root)
                }) {
                    self.selected_agent = row;
                    return;
                }
            }
        }
        let len = self.project_rows().len();
        self.selected_agent = self.selected_agent.min(len.saturating_sub(1));
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

    /// The model and route Gate sends the next turn to, `None` for a
    /// pinned model.
    #[must_use]
    pub fn model_route(&self) -> Option<&str> {
        self.model_route.as_deref()
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

    /// Labels of the usable linked plans (e.g. "Claude").
    #[must_use]
    pub fn plans(&self) -> &[String] {
        &self.plans
    }

    /// Work mode of the agent on screen.
    #[must_use]
    pub fn mode(&self) -> crate::mode_command::PermissionMode {
        self.hud.mode.unwrap_or_default()
    }

    /// Approval level of the agent on screen; `None` until the event loop
    /// reports it.
    #[must_use]
    pub const fn approval(&self) -> Option<crate::mode_command::ApprovalLevel> {
        self.hud.approval
    }

    /// The active agent's current plan.
    #[must_use]
    pub const fn active_plan(&self) -> Option<&PlanView> {
        self.active_plan.as_ref()
    }

    /// Bash-style `[user@host dir]$ ` prompt shown before the input line at
    /// the terminal prompt.
    #[must_use]
    pub fn shell_prompt(&self) -> &str {
        &self.shell_prompt
    }

    #[cfg(test)]
    pub(crate) fn set_shell_prompt(&mut self, prompt: &str) {
        self.shell_prompt = prompt.to_string();
    }

    /// Whether the input line shows the terminal prompt and Enter runs it
    /// as a shell command. Prompts that ask for a value (login, project
    /// name, folder) take the line over while they are open.
    #[must_use]
    pub const fn input_is_command(&self) -> bool {
        self.terminal_mode
            && !matches!(
                self.state,
                AppState::LoginEmail
                    | AppState::LoginPassword
                    | AppState::LinkCode
                    | AppState::ProjectName
                    | AppState::SelectingFolder
            )
    }

    /// Record the width of the chat text area drawn this frame.
    pub fn set_chat_width(&mut self, width: u16) {
        self.chat_width = width;
    }

    /// Width of the chat text area drawn last frame.
    #[cfg(test)]
    pub(crate) const fn chat_width(&self) -> u16 {
        self.chat_width
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

    /// Current frame of the ring shown beside thinking agents.
    #[must_use]
    pub fn aura_frame(&self) -> (String, ratatui::style::Color) {
        crate::animation::aura_ring(self.animation_frame)
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

    /// Record or file shown over the chat panel, if any.
    #[must_use]
    pub const fn detail_view(&self) -> Option<&DetailView> {
        self.detail.as_ref()
    }

    /// Show `view` over the chat panel, starting at its top.
    pub(super) fn open_detail(&mut self, view: DetailView) {
        self.detail = Some(view);
        self.detail_scroll = 0;
        self.graph.behind = false;
    }

    /// Close the view over the chat panel.
    pub(super) fn close_detail(&mut self) {
        self.detail = None;
        self.detail_scroll = 0;
        self.graph.behind = false;
    }

    /// Open the highlighted sandbox row: a file over the chat panel; a
    /// directory, which has nothing to show, folds or unfolds.
    pub(super) fn activate_selected_sandbox_row(&mut self) {
        let Some(row) = self.sandbox.rows().get(self.sandbox.selected()) else {
            return;
        };
        match row.kind {
            SandboxEntryKind::File | SandboxEntryKind::Symlink => {
                let view = FileView::load(&row.path, self.sandbox.root());
                self.open_detail(DetailView::File(Box::new(view)));
            }
            SandboxEntryKind::Dir => self.sandbox.toggle_selected(),
            SandboxEntryKind::More(_) => {}
        }
    }

    /// Unfold the highlighted directory; on an open directory or a file,
    /// do nothing.
    pub(super) fn expand_selected_sandbox_dir(&mut self) {
        let closed_dir = self
            .sandbox
            .rows()
            .get(self.sandbox.selected())
            .is_some_and(|row| row.kind == SandboxEntryKind::Dir && !row.expanded);
        if closed_dir {
            self.sandbox.toggle_selected();
        }
    }

    pub(super) fn scroll_detail_up(&mut self, lines: usize) {
        self.detail_scroll = self.detail_scroll.saturating_sub(lines);
        self.detail_scrolled();
    }

    pub(super) fn scroll_detail_down(&mut self, lines: usize) {
        self.detail_scroll = self.detail_scroll.saturating_add(lines);
        self.detail_scrolled();
    }

    /// An open file's cursor follows a scroll instead of pulling it back.
    fn detail_scrolled(&mut self) {
        if let Some(DetailView::File(file)) = self.detail.as_mut() {
            file.vim.scrolled();
        }
    }

    /// Lines a page key moves the detail view.
    pub(super) fn detail_page_rows(&self) -> usize {
        self.detail_shown.saturating_sub(1).max(1)
    }

    /// The open file and its scroll, for keys and drawing.
    pub(crate) fn file_view_mut(&mut self) -> Option<(&mut FileView, &mut usize)> {
        match self.detail.as_mut() {
            Some(DetailView::File(file)) => Some((file.as_mut(), &mut self.detail_scroll)),
            _ => None,
        }
    }

    /// The open image, to draw it.
    pub(crate) fn image_view_mut(&mut self) -> Option<&mut image_preview::ImageView> {
        match self.detail.as_mut() {
            Some(DetailView::Image(view)) => Some(view.as_mut()),
            _ => None,
        }
    }

    /// What the detail view showed in the last frame.
    pub(super) const fn detail_screen(&self) -> vim::Screen {
        vim::Screen {
            first: self.detail_scroll,
            shown: self.detail_shown,
        }
    }

    /// First line of the detail view to show.
    #[must_use]
    pub const fn detail_scroll(&self) -> usize {
        self.detail_scroll
    }

    /// Clamp the detail view's scroll to `max_scroll` and record that
    /// `shown` source lines fit, as measured by the last frame.
    pub fn set_detail_bounds(&mut self, max_scroll: usize, shown: usize) {
        self.detail_scroll = self.detail_scroll.min(max_scroll);
        self.detail_shown = shown.max(1);
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

    /// Drop the copy confirmation, if it has expired.
    fn expire_copy_notice(&mut self) {
        if self
            .copy_notice
            .as_ref()
            .is_some_and(|(_, _, until)| Instant::now() >= *until)
        {
            self.copy_notice = None;
        }
    }

    /// Cancel the turn of the agent on screen.
    pub fn cancel(&mut self) {
        if self.is_busy() {
            // The event loop denies the waiting call when it cancels the
            // turn, so the prompts can go now.
            for approval in std::mem::take(&mut self.pending_approvals) {
                self.mark_awaiting_approval(&approval.id, false);
            }
            self.state = AppState::Idle;
            self.status = "Cancelled".to_string();
            if let Some(tx) = &self.event_tx {
                let _ = tx.try_send(UiEvent::Cancel(self.active_agent_id.clone()));
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

    /// Request a switch to another agent. Agents keep working while
    /// another one is shown.
    pub(super) fn request_agent_switch(&mut self, agent_id: String) {
        if agent_id == self.active_agent_id {
            return;
        }
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::SelectAgent(agent_id));
        }
    }

    /// Process pending updates from the command channel.
    pub fn tick(&mut self) {
        self.animation_frame = self.animation_frame.wrapping_add(1);
        self.expire_copy_notice();

        self.receive_commands();
        if std::mem::take(&mut self.sandbox_stale) && self.sandbox_panel_visible {
            self.sandbox.refresh();
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

    /// Apply every command waiting in the command channel. Between frames
    /// this keeps the channel from filling, which would hold up the agents
    /// and the event loop sending to it.
    pub fn receive_commands(&mut self) {
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
