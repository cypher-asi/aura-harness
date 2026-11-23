//! Application state machine for the terminal UI.
//!
//! Manages the overall application state, conversation history,
//! panels (Chat, Record and Sandbox), and coordinates between input handling and rendering.

mod agent_views;
mod archive;
pub(crate) mod bench_progress;
pub mod benchmarks;
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
mod inbox;
mod input_editor;
mod keys;
mod line_edit;
mod members;
pub mod mentions;
mod model_picker;
mod panes;
pub mod plans;
mod policies;
mod processes;
mod project_drag;
mod prose;
pub(crate) mod queue_panel;
pub mod run_log;
pub mod tasks;
pub use aura_ui::sandbox;
pub(crate) mod scrollbar;
mod selection;
pub(crate) mod shell_input;
mod shell_prompt;
mod side_scope;
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
mod queue_panel_tests;
#[cfg(test)]
mod tests;

use crate::{
    alerts::AlertSettings,
    components::{Activity, Message},
    events::{
        empty_projects, starts_project, AgentDefinitionSummary, AgentSummary, ModelOptionView,
        PlanView, ProcessSummary, ProjectSummary, RecordSummary, UiCommand, UiEvent,
    },
    input::InputHistory,
    renderer::{ChatLineCache, GroupState},
};
pub use chat_tabs::ChatTab;
pub(crate) use context_menu::MenuTarget;
pub use context_menu::{WorkspaceMove, WorkspaceTarget};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
pub use definitions::AgentsTab;
use file_view::FileView;
use folder_picker::FolderPicker;
use hud::Hud;
pub use inbox::{InboxRow, ThreadView};
pub use members::{MemberRow, RecordsTab};
pub use model_picker::{ModelPicker, ModelTab};
pub use panes::{Pane, PaneChrome, PaneSplits, PaneTab};
pub use processes::SidePanelTab;
pub use prose::{ProseMeasure, ProseRow};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use sandbox::{SandboxEntryKind, SandboxTree};
pub use selection::Selection;
pub use side_scope::SideScope;
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

/// How long a notification stays in the corner.
const NOTIFICATION_TTL: Duration = Duration::from_secs(4);

/// How long an error stays in the corner.
const ERROR_NOTIFICATION_TTL: Duration = Duration::from_secs(6);

/// How long a notification too long for its row stays once it has
/// scrolled to its end.
const NOTIFICATION_TAIL: Duration = Duration::from_secs(2);

// Maximum number of records to keep in the list, shared with the loader.
use crate::events::MAX_RECORDS;

/// Time between steps of the chat scrolling under a selection dragged
/// past its top or bottom.
const SELECTION_SCROLL_EVERY: Duration = Duration::from_millis(40);

/// Most rows the chat scrolls in one step under a dragged selection; it
/// scrolls a row per row the pointer is past the edge, up to this.
const SELECTION_SCROLL_MOST: u16 = 8;

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
    /// Model selector overlay is open
    SelectingModel,
    /// Folder browser for a new project is open
    SelectingFolder,
    /// A project's details are shown over the screen
    ShowingProjectInfo,
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
    /// Listed projects, in list order
    pub(super) projects: Vec<ProjectSummary>,
    /// Key of the project whose details are shown
    pub(super) project_info: Option<String>,
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
    /// The notification on screen and when it goes away.
    pub(super) notification_shown: Option<((String, NotificationType), Instant)>,
    /// When the notification on screen first showed.
    pub(super) notification_since: Option<Instant>,
    /// Which panel has focus
    pub(super) focus: PanelFocus,
    /// Whether the Record panel is visible
    pub(super) record_panel_visible: bool,
    /// Whether the Swarm panel is visible
    pub(super) swarm_panel_visible: bool,
    /// Whether the Sandbox panel is visible
    pub(super) sandbox_panel_visible: bool,
    /// Whether the chat panel fills the screen, the other panels hidden
    pub(super) chat_expanded: bool,
    /// Whether the side panel fills the right column, over the Record
    /// panel, while it is shown
    pub(super) side_expanded: bool,
    /// File tree of the active agent's sandbox
    pub(super) sandbox: SandboxTree,
    /// Whether a tool may have changed files since the tree was read; the
    /// next tick reads it again, once however many tools finished
    pub(super) sandbox_stale: bool,
    /// Animation frame counter for spinners
    pub(super) animation_frame: usize,
    /// Whether the screen may look different since the host last drew
    /// it; see [`Self::take_redraw`]
    pub(super) redraw: bool,
    /// Kernel records list
    pub(super) records: VecDeque<RecordSummary>,
    /// Selected record index in the list
    pub(super) selected_record: usize,
    /// Scroll offset for records list
    pub(super) records_scroll: usize,
    /// Which list the Kernel panel shows
    pub(super) records_tab: RecordsTab,
    /// The agents of the run of the agent on screen, for the MEMBERS tab
    pub(super) members: members::Members,
    /// The threads of mail between agents, for the INBOX tab
    pub(super) inbox: inbox::Inbox,
    /// First line of the POLICIES tab on screen
    pub(super) policies_scroll: usize,
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
    /// Ids of agents folded shut, hiding the agents they spawned
    pub(super) collapsed_agents: HashSet<String>,
    /// The left button is down on a project's header: a click folds it, a
    /// drag moves it
    pub(super) project_press: Option<project_drag::ProjectPress>,
    /// Times a drag has reordered the projects, so the screen is redrawn
    pub(super) project_moves: usize,
    /// Currently active agent ID
    pub(super) active_agent_id: String,
    /// Chats of the agents not on screen, by agent ID
    pub(super) views: HashMap<String, agent_views::AgentView>,
    /// Whether the chat on screen holds its agent's history, so showing
    /// it again needs no reload (see `agent_views`)
    pub(super) chat_loaded: bool,
    /// When the chat on screen started waiting for its history
    pub(super) chat_loading_since: Option<Instant>,
    /// When the chat on screen got its history after waiting for it, to
    /// fade it in
    pub(super) chat_revealed_at: Option<Instant>,
    /// The agent the UI switched to on screen and asked the session for,
    /// until the session confirms it; echoes of earlier switches are
    /// ignored meanwhile
    pub(super) switch_sent: Option<String>,
    /// The agent the session last confirmed as active, to go back to when
    /// a switch fails
    pub(super) confirmed_agent: String,
    /// What was typed for the agents not on screen, by agent ID; each
    /// agent gets its draft back when it comes on screen again
    pub(super) drafts: HashMap<String, queue_panel::Draft>,
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
    /// Thinking level pinned for every turn; `None` when each model picks
    pub(super) thinking: Option<String>,
    /// Logged-in user's display name (`None` = guest)
    pub(super) user: Option<String>,
    /// Labels of the usable linked plans
    pub(super) plans: Vec<String>,
    /// The active agent's current plan
    pub(super) active_plan: Option<PlanView>,
    /// Model selector state: tab, filter and highlighted row
    pub(super) model_picker: ModelPicker,
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
    /// Agent (hex id) whose queue the last key put a message in. The next
    /// key, if it is Enter on an empty prompt, sends that message now even
    /// before the event loop's `SetQueue` shows it, and whatever panel has
    /// the keys; any other key clears it.
    pub(super) just_queued: Option<String>,
    /// Image thumbnails in the chat in the last frame
    pub(super) image_hits: Vec<ImageHit>,
    /// Thumbnails of the images waiting in the input, in the last frame
    pub(super) pending_image_areas: Vec<(Rect, usize)>,
    /// Absolute cell rect where the detail-view image was drawn in the last
    /// frame; `None` when no image detail view is open or nothing was drawn.
    pub(super) detail_image_rect: Option<Rect>,
    /// Text selected with the mouse
    pub(super) selection: Option<Selection>,
    /// Where the pointer is while the left button is still held for
    /// `selection`
    pub(super) selecting: Option<Position>,
    /// The chat panel's scrollbar while the left button holds it
    pub(super) scrollbar_grab: Option<scrollbar::ScrollbarGrab>,
    /// The cells of the chat panel's scrollbar track as it was drawn this
    /// frame, to tell what was drawn over it after
    pub(super) scrollbar_cells: Vec<ratatui::buffer::Cell>,
    /// Whether `selection` is in the chat panel's body, whose rows scroll
    pub(super) selection_scrolls: bool,
    /// When the chat last scrolled for a selection dragged past its edge
    pub(super) selection_scrolled: Option<Instant>,
    /// Whether the selection should be copied after the next frame.
    /// Dropping the selection drops this, not `copy_pending_text`.
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
    /// The host's workspaces, which the menu offers to move a project to
    pub(super) workspaces: context_menu::Workspaces,
    /// A move to another workspace the menu asked for, until the host
    /// takes it
    pub(super) workspace_move: Option<WorkspaceMove>,
    /// Usage, todos and jobs of the agent on screen
    pub(super) hud: Hud,
    /// The queued-messages panel above the input
    pub(super) queue_panel: queue_panel::QueuePanel,
    /// The benchmark progress panel above the input
    pub(super) bench_panel: bench_progress::BenchPanel,
    /// Whether the full todo list is shown above the input
    pub(super) todos_open: bool,
    /// When to get the user's attention
    pub(super) alert_settings: AlertSettings,
    /// Messages for the user that the terminal should ring the bell for
    pub(super) alerts: Vec<String>,
    /// Which view the panel below the Record panel shows
    pub(super) side_tab: SidePanelTab,
    /// Whose items the panel's lists show
    pub(super) side_scope: SideScope,
    /// Every process, of every agent
    pub(super) all_processes: Vec<ProcessSummary>,
    /// Agents whose processes the Processes tab lists
    pub(super) process_filter: side_scope::AgentFilter,
    /// Rows of the Processes tab: the processes `process_filter` lets
    /// through
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
    /// Benchmarks shown by the chat panel's BENCH tab
    pub(super) bench: benchmarks::BenchState,
    /// Run log shown by the chat panel's LOG tab
    pub(super) log: run_log::LogState,
    /// Text to copy after the next frame instead of the selection
    pub(super) copy_pending_text: Option<String>,
    /// What that text is, for the notice once it is copied
    pub(super) copy_label: Option<String>,
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
    /// Close button in the chat panel's header bar while a view covers it
    pub detail_close: Option<Rect>,
    /// Button in the chat panel's header bar that expands or restores it
    pub chat_expand: Option<Rect>,
    /// Message text area inside the chat panel
    pub chat_text: Option<Rect>,
    /// Rows of the chat panel's body that scroll by rows: the
    /// conversation, the terminal, the benchmarks, or a record or file
    /// shown over them. `None` under the graph or an image.
    pub chat_rows: Option<Rect>,
    /// Content row shown at the top of `chat_rows`
    pub chat_rows_top: usize,
    /// Input box under the chat panel (including border)
    pub input: Option<Rect>,
    /// Stop button at the right end of the input box's first row, drawn
    /// while the agent on screen is busy
    pub input_stop: Option<Rect>,
    /// Record panel (including border)
    pub records: Option<Rect>,
    /// Header bar of the record panel
    pub record_tabs: Option<Rect>,
    /// Rows of the record list inside the panel
    pub record_rows: Option<Rect>,
    /// Rows of the MEMBERS tab inside the record panel
    pub member_rows: Option<Rect>,
    /// Member row shown at the top of `member_rows`
    pub members_scroll: usize,
    /// Rows of the INBOX tab inside the record panel
    pub inbox_rows: Option<Rect>,
    /// Inbox row shown at the top of `inbox_rows`
    pub inbox_scroll: usize,
    /// Sandbox panel (including border)
    pub sandbox: Option<Rect>,
    /// Rows of the sandbox tree inside the panel
    pub sandbox_rows: Option<Rect>,
    /// Tree row shown at the top of `sandbox_rows`
    pub sandbox_scroll: usize,
    /// Header bar of the panel below the Record panel, up to its `*`
    /// button
    pub side_tabs: Option<Rect>,
    /// Button at the right end of that bar that shows the whole
    /// project's items in its lists, or the active agent's
    pub side_scope: Option<Rect>,
    /// Button right of that one that maximizes the panel over the Record
    /// panel, or restores it
    pub side_expand: Option<Rect>,
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
    /// Scrollbar on the chat panel's right border, when its list
    /// overflows
    pub(crate) scrollbar: Option<scrollbar::ScrollbarArea>,
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
    /// A thread of mail between agents, from the INBOX tab, kept up to
    /// date as its messages arrive.
    Thread(Box<ThreadView>),
}

/// Clickable item under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoverTarget {
    /// A label in the agents tab bar.
    AgentTab(AgentsTab),
    /// A label in the chat panel's tab bar.
    ChatTab(ChatTab),
    /// The close button of the view over the chat panel.
    CloseDetail,
    /// The button that expands the chat panel or restores it.
    ExpandChat,
    /// The stop button in the input box, shown while a turn runs.
    StopTurn,
    /// A row in the projects list, including "+ New project".
    ProjectRow(usize),
    /// The "+" at the right end of a project's row in the projects list.
    AddToProject(usize),
    /// The archive button left of the time on an agent's row in the
    /// projects list, shown while the row is hovered.
    ArchiveAgent(usize),
    /// A row in the roster, including "+ New agent".
    DefinitionRow(usize),
    /// A row in the Archive.
    ArchivedRow(usize),
    /// A row in the sandbox tree.
    SandboxRow(usize),
    /// A row in the Record panel.
    RecordRow(usize),
    /// A label in the Record panel's tab bar.
    RecordTab(RecordsTab),
    /// A row in the Record panel's MEMBERS tab.
    MemberRow(usize),
    /// A row in the Record panel's INBOX tab.
    InboxRow(usize),
    /// A label in the side panel's tab bar.
    SideTab(SidePanelTab),
    /// The `*` button at the right end of the side panel's tab bar.
    SideScope,
    /// The button right of it that maximizes the side panel or restores
    /// it.
    ExpandSide,
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

/// Glyph of the archive button drawn on a hovered agent's row in the
/// projects list. The desktop draws it as a shape, so it lines up with the
/// cell grid there as well.
pub(crate) const AGENT_ARCHIVE_GLYPH: &str = "✕";

/// A button drawn on a hovered agent's row in the projects list. Clicking
/// elsewhere on the row already shows the agent, so there is no button
/// for that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentButton {
    /// Archives the agent.
    Archive,
}

impl AgentButton {
    /// The buttons, left to right as drawn.
    pub(crate) const ALL: [Self; 1] = [Self::Archive];

    pub(crate) const fn glyph(self) -> &'static str {
        match self {
            Self::Archive => AGENT_ARCHIVE_GLYPH,
        }
    }

    /// What the button draws: a space, then its glyph.
    pub(crate) fn label(self) -> String {
        format!(" {}", self.glyph())
    }

    /// The hover target of this button on projects-list row `index`.
    pub(crate) const fn target(self, index: usize) -> HoverTarget {
        match self {
            Self::Archive => HoverTarget::ArchiveAgent(index),
        }
    }
}

/// The fewest columns an agent's name keeps beside its code; narrower rows
/// drop the code instead.
const MIN_NAME_BESIDE_CODE: usize = 4;

/// Columns before the name on the row of an agent at `depth`: one leading
/// space, the spawn guides at `AURA_WIDTH + 2` columns per level, then the
/// icon column and the space after it, as `tree_row` lays them out.
fn agent_row_lead(depth: usize) -> usize {
    use crate::animation::AURA_WIDTH;
    1 + depth * (AURA_WIDTH + 2) + AURA_WIDTH + 1
}

/// What a projects-list or roster row shows right of its buttons, flush
/// with the right edge: the time cell, then the code cell (on roster rows,
/// the definition's abbreviation).
///
/// The time cell is the time with a space on each side (` 5m `). With no
/// time it is a single space, which keeps the last button or the code off
/// the time's place, or nothing on a row with neither buttons nor a code
/// column. The code cell is the agent's code, left-aligned and padded to
/// the code column's width so codes line up, then one space before the
/// panel's edge; with no code column it is empty.
///
/// The renderer draws these cells and the hit-testing finds the buttons
/// from them, so the two cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AgentRowEnd<'a> {
    ago: Option<&'a str>,
    code: Option<&'a str>,
    code_width: usize,
}

impl<'a> AgentRowEnd<'a> {
    /// The end of a row with a time and no code column, as project and
    /// archive rows have.
    pub(crate) const fn time(ago: Option<&'a str>) -> Self {
        Self {
            ago,
            code: None,
            code_width: 0,
        }
    }

    /// The end of an agent row: the time alone. Agent rows have no code
    /// column; only roster rows show a code (the definition's
    /// abbreviation), so names get the whole width.
    pub(crate) fn agent(ago: Option<&'a str>) -> Self {
        Self::time(ago)
    }

    /// The end of a roster row, in a list `width` columns wide whose
    /// abbreviation column is `column_width` columns (0 for none): no
    /// time, then the definition's `abbreviation` laid out as a code. The
    /// column is dropped when it would leave the name fewer than
    /// [`MIN_NAME_BESIDE_CODE`] columns. Roster rows have no buttons.
    pub(crate) fn definition(
        width: usize,
        (abbreviation, column_width): (Option<&'a str>, usize),
    ) -> Self {
        Self::coded_if_room(
            width,
            agent_row_lead(0),
            None,
            (abbreviation, column_width),
            false,
        )
    }

    /// The end with `code` in a column `code_width` wide when the row,
    /// `width` columns with `taken` of them before the name and after it
    /// besides this end, leaves the name at least [`MIN_NAME_BESIDE_CODE`]
    /// columns; else the time alone.
    fn coded_if_room(
        width: usize,
        taken: usize,
        ago: Option<&'a str>,
        (code, code_width): (Option<&'a str>, usize),
        has_buttons: bool,
    ) -> Self {
        if code_width == 0 {
            return Self::time(ago);
        }
        let with_code = Self {
            ago,
            code,
            code_width,
        };
        let around = taken + text_width(&with_code.text(has_buttons));
        if width.saturating_sub(around) >= MIN_NAME_BESIDE_CODE {
            with_code
        } else {
            Self::time(ago)
        }
    }

    /// The time cell, for a row whose buttons show when `has_buttons`.
    pub(crate) fn time_cell(&self, has_buttons: bool) -> String {
        match self.ago {
            Some(ago) => format!(" {ago} "),
            None if has_buttons || self.code_width > 0 => " ".to_string(),
            None => String::new(),
        }
    }

    /// The code cell: empty with no code column.
    pub(crate) fn code_cell(&self) -> String {
        if self.code_width == 0 {
            return String::new();
        }
        let code = self.code.unwrap_or_default();
        let pad = self.code_width.saturating_sub(text_width(code));
        format!("{code}{} ", " ".repeat(pad))
    }

    /// The whole text right of the buttons: the time cell, then the code
    /// cell.
    pub(crate) fn text(&self, has_buttons: bool) -> String {
        self.time_cell(has_buttons) + &self.code_cell()
    }
}

fn text_width(text: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    text.width()
}

fn columns(text: &str) -> u16 {
    u16::try_from(text_width(text)).unwrap_or(u16::MAX)
}

/// Column of `button`'s glyph on an agent's row in the projects list,
/// counted from the row's left edge, for a row `width` columns wide that
/// ends in `end`. The renderer lays the row out from the same pieces:
/// each button's [`AgentButton::label`] in [`AgentButton::ALL`] order,
/// then [`AgentRowEnd::text`], flush with the right edge.
pub(crate) fn agent_button_offset(width: u16, end: &AgentRowEnd<'_>, button: AgentButton) -> u16 {
    let after: u16 = AgentButton::ALL
        .iter()
        .skip_while(|&&b| b != button)
        .skip(1)
        .map(|b| columns(&b.label()))
        .fold(0, u16::saturating_add);
    let glyph = columns(button.glyph());
    width.saturating_sub(
        columns(&end.text(true))
            .saturating_add(after)
            .saturating_add(glyph),
    )
}

/// The button at column `offset` of an agent's row laid out as
/// [`agent_button_offset`] says. Each button takes its glyph and the space
/// left of it, so the zones don't overlap; the last also takes the space
/// between it and the time.
pub(crate) fn agent_button_at(
    width: u16,
    end: &AgentRowEnd<'_>,
    offset: u16,
) -> Option<AgentButton> {
    let last = AgentButton::ALL.len() - 1;
    AgentButton::ALL
        .iter()
        .enumerate()
        .find(|&(i, &button)| {
            let glyph = agent_button_offset(width, end, button);
            let end = glyph.saturating_add(columns(button.glyph()));
            let end = if i == last {
                end.saturating_add(1)
            } else {
                end
            };
            (glyph.saturating_sub(1)..end).contains(&offset)
        })
        .map(|(_, &button)| button)
}

/// One row of the projects list. A project is its own row; its first
/// agent, and any agents that agent spawned, hang underneath while the
/// project is expanded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectListRow {
    /// Foldable header. `root` is the index of the project's first agent.
    Project { root: usize, expanded: bool },
    /// Header of a project with every agent archived, which stays listed
    /// until it is removed. `project` is its index in
    /// [`App::projects`].
    EmptyProject { project: usize },
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
            projects: Vec::new(),
            project_info: None,
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
            notification_shown: None,
            notification_since: None,
            focus: PanelFocus::default(),
            record_panel_visible: true,
            swarm_panel_visible: true,
            sandbox_panel_visible: true,
            chat_expanded: false,
            side_expanded: false,
            sandbox: SandboxTree::default(),
            sandbox_stale: false,
            animation_frame: 0,
            redraw: true,
            records: VecDeque::new(),
            selected_record: 0,
            records_scroll: 0,
            records_tab: RecordsTab::default(),
            members: members::Members::default(),
            inbox: inbox::Inbox::default(),
            policies_scroll: 0,
            detail: None,
            detail_scroll: 0,
            detail_shown: 1,
            agents: Vec::new(),
            selected_agent: 0,
            collapsed_projects: HashSet::new(),
            collapsed_agents: HashSet::new(),
            project_press: None,
            project_moves: 0,
            active_agent_id: String::new(),
            views: HashMap::new(),
            // The chat at startup gets its history without a switch; it
            // shows at once instead of fading in.
            chat_loaded: true,
            chat_loading_since: None,
            chat_revealed_at: None,
            switch_sent: None,
            confirmed_agent: String::new(),
            drafts: HashMap::new(),
            api_url: None,
            api_active: false,
            login_email: String::new(),
            models: Vec::new(),
            current_model: String::new(),
            model_route: None,
            thinking: None,
            user: None,
            plans: Vec::new(),
            active_plan: None,
            model_picker: ModelPicker::default(),
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
            just_queued: None,
            image_hits: Vec::new(),
            pending_image_areas: Vec::new(),
            detail_image_rect: None,
            selection: None,
            selecting: None,
            scrollbar_grab: None,
            scrollbar_cells: Vec::new(),
            selection_scrolls: false,
            selection_scrolled: None,
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
            workspaces: context_menu::Workspaces::default(),
            workspace_move: None,
            hud: Hud::default(),
            queue_panel: queue_panel::QueuePanel::default(),
            bench_panel: bench_progress::BenchPanel::default(),
            todos_open: false,
            alert_settings: AlertSettings::default(),
            alerts: Vec::new(),
            side_tab: SidePanelTab::default(),
            side_scope: SideScope::default(),
            all_processes: Vec::new(),
            process_filter: None,
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
            bench: benchmarks::BenchState::default(),
            log: run_log::LogState::default(),
            copy_pending_text: None,
            copy_label: None,
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
            PanelFocus::Chat if self.bench.is_open() => {
                self.bench
                    .scroll_by(-isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Chat if self.log.is_open() && self.detail.is_none() => {
                self.log
                    .scroll_by(-isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Chat => {
                self.scroll_offset = self.scroll_offset.saturating_add(lines);
            }
            PanelFocus::Records if self.records_tab == RecordsTab::Members => {
                self.scroll_members(-isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Records if self.records_tab == RecordsTab::Policies => {
                self.scroll_policies(-isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Records if self.records_tab == RecordsTab::Inbox => {
                self.scroll_inbox(-isize::try_from(lines).unwrap_or(isize::MAX));
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
            PanelFocus::Chat if self.bench.is_open() => {
                self.bench
                    .scroll_by(isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Chat if self.log.is_open() && self.detail.is_none() => {
                self.log
                    .scroll_by(isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Chat => {
                self.scroll_offset = self.scroll_offset.saturating_sub(lines);
            }
            PanelFocus::Records if self.records_tab == RecordsTab::Members => {
                self.scroll_members(isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Records if self.records_tab == RecordsTab::Policies => {
                self.scroll_policies(isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Records if self.records_tab == RecordsTab::Inbox => {
                self.scroll_inbox(isize::try_from(lines).unwrap_or(isize::MAX));
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
            Some(ProjectListRow::EmptyProject { .. } | ProjectListRow::Gap) => {}
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

    /// Ask for a folder, then open it as a project named after it.
    /// Browsing starts in the launch directory.
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

    /// Open the folder browser for a new project, as Ctrl+N does.
    pub fn ask_for_project_folder(&mut self) {
        self.prompt_new_project();
    }

    /// Close the folder browser and open the folder as a project.
    pub(super) fn choose_project_folder(&mut self, folder: PathBuf) {
        self.folder_picker = None;
        self.state = AppState::Idle;
        self.status = "Ready".to_string();
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::CreateProject { folder });
        }
    }

    /// Close the folder browser without creating a project.
    pub(super) fn cancel_folder_picker(&mut self) {
        self.folder_picker = None;
        self.state = AppState::Idle;
        self.status = "Ready".to_string();
        self.notification = Some(("Cancelled".to_string(), NotificationType::Warning));
    }

    /// A path was dropped on the window. A folder becomes a project named
    /// after it, with the same event the folder browser sends; anything
    /// else is refused with a notice. Returns whether a project was asked
    /// for.
    pub fn drop_path(&mut self, path: &std::path::Path) -> bool {
        if !path.is_dir() {
            self.notification = Some((
                "Drop a folder to start a project".to_string(),
                NotificationType::Warning,
            ));
            return false;
        }
        let Some(tx) = &self.event_tx else {
            return false;
        };
        let event = UiEvent::CreateProject {
            folder: path.to_path_buf(),
        };
        if tx.try_send(event).is_err() {
            self.notification = Some((
                "Cannot add the project right now".to_string(),
                NotificationType::Error,
            ));
            return false;
        }
        true
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

    /// Listed projects, in list order.
    #[must_use]
    pub fn projects(&self) -> &[ProjectSummary] {
        &self.projects
    }

    /// The project whose details are shown, while they are.
    #[must_use]
    pub fn project_info(&self) -> Option<&ProjectSummary> {
        if self.state != AppState::ShowingProjectInfo {
            return None;
        }
        let key = self.project_info.as_deref()?;
        self.projects.iter().find(|project| project.id == key)
    }

    /// Show the details of the project with key `key`.
    pub(super) fn show_project_info(&mut self, key: &str) {
        if !self.projects.iter().any(|project| project.id == key) {
            self.notification = Some((
                "No details for that project".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        self.project_info = Some(key.to_string());
        self.state = AppState::ShowingProjectInfo;
    }

    /// Show the details of the project the active agent is in.
    pub(super) fn show_active_project_info(&mut self) {
        match self.project_key_of_agent(&self.active_agent_id) {
            Some(key) => self.show_project_info(&key),
            None => {
                self.notification =
                    Some(("No project is open".to_string(), NotificationType::Warning));
            }
        }
    }

    /// Close the project details.
    pub(super) fn close_project_info(&mut self) {
        self.project_info = None;
        self.state = AppState::Idle;
    }

    /// Fold or open what is under the highlight: an agent with spawns
    /// folds over them, any other agent row folds the project that
    /// contains it, as does a project's header.
    pub(super) fn toggle_selected_fold(&mut self) {
        self.agents_scroll_detached = false;
        let root = match self.project_rows().get(self.selected_agent).copied() {
            Some(ProjectListRow::Project { root, .. }) => Some(root),
            Some(ProjectListRow::Agent { index }) if self.agent_has_spawns(index) => {
                self.toggle_agent(index);
                return;
            }
            Some(ProjectListRow::Agent { index }) => self.project_root(index),
            Some(
                ProjectListRow::EmptyProject { .. }
                | ProjectListRow::NewProject
                | ProjectListRow::Gap,
            )
            | None => None,
        };
        if let Some(root) = root {
            self.toggle_project(root);
        }
    }

    /// Fold or open the project that contains the highlighted row, from
    /// any agent in it or its header, even an agent that could fold over
    /// its own spawns.
    pub(super) fn toggle_selected_project(&mut self) {
        self.agents_scroll_detached = false;
        let root = match self.project_rows().get(self.selected_agent).copied() {
            Some(ProjectListRow::Project { root, .. }) => Some(root),
            Some(ProjectListRow::Agent { index }) => self.project_root(index),
            Some(
                ProjectListRow::EmptyProject { .. }
                | ProjectListRow::NewProject
                | ProjectListRow::Gap,
            )
            | None => None,
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

    /// Record the rows the chat panel's body scrolls through this frame
    /// and the content row on top of them, or that it shows nothing that
    /// scrolls by rows.
    pub(crate) fn set_chat_rows(&mut self, rows: Option<(Rect, usize)>) {
        self.areas.chat_rows = rows.map(|(area, _)| area);
        self.areas.chat_rows_top = rows.map_or(0, |(_, top)| top);
    }

    /// Record where the input box was drawn this frame.
    pub fn set_input_area(&mut self, area: Rect) {
        self.areas.input = Some(area);
    }

    /// Record where the input box's stop button was drawn this frame, or
    /// that it was not.
    pub fn set_input_stop_area(&mut self, area: Option<Rect>) {
        self.areas.input_stop = area;
    }

    /// Where the input box's stop button was drawn last frame.
    #[cfg(test)]
    pub(crate) const fn input_stop_area(&self) -> Option<Rect> {
        self.areas.input_stop
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
    /// drag started in and copies it when the button is released; in the
    /// chat panel, unless it shows the graph or an image, holding the
    /// pointer past its top or bottom scrolls it (see [`Self::tick`]), and
    /// the wheel scrolls it under the selection. A press on the chat
    /// panel's scrollbar takes hold of it instead, and dragging then
    /// scrolls what the panel shows.
    /// Moving over a clickable item highlights it until the pointer leaves.
    ///
    /// Returns whether the screen may look different: false for a move or
    /// a drag that left the highlight and the selection as they were.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        let pointing = matches!(mouse.kind, MouseEventKind::Moved | MouseEventKind::Drag(_));
        let menu = |app: &Self| {
            app.context_menu.as_ref().map(|menu| {
                let submenu = menu.submenu.as_ref().map(|submenu| submenu.selected);
                (menu.selected, submenu)
            })
        };
        let ends = |app: &Self| app.selection.as_ref().map(Selection::ends);
        let state = |app: &Self| {
            (
                app.hover,
                ends(app),
                menu(app),
                app.project_moves,
                app.project_drag_hit(),
                app.queue_pointer_state(),
                (app.scrollbar_held(), app.scroll_offset, app.detail_scroll),
            )
        };
        let before = state(self);
        self.mouse(mouse);
        !pointing || before != state(self)
    }

    fn mouse(&mut self, mouse: MouseEvent) {
        // First, so the button works while an approval waits too; an
        // open context menu takes the click instead, as it may cover it.
        if self.context_menu.is_none() && self.click_stop_button(mouse) {
            return;
        }
        if self.state != AppState::Idle && self.state != AppState::Processing {
            self.hover = None;
            self.context_menu = None;
            self.project_press = None;
            self.scrollbar_grab = None;
            self.drop_queue_press();
            return;
        }
        if self.context_menu_mouse(mouse.kind, mouse.column, mouse.row) {
            self.hover = None;
            return;
        }
        if self.queue_mouse(mouse.kind, mouse.column, mouse.row)
            || self.bench_panel_mouse(mouse.kind, mouse.column, mouse.row)
        {
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
        // Right-click in the input box pastes, as in a terminal.
        if mouse.kind == MouseEventKind::Down(MouseButton::Right)
            && contains(self.areas.input, mouse.column, mouse.row)
        {
            self.clear_selection();
            self.hover = None;
            self.request_paste();
            return;
        }
        if self.scrollbar_mouse(mouse.kind, mouse.column, mouse.row) {
            self.hover = self.hover_target(mouse.column, mouse.row);
            return;
        }
        let at = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                self.hover = None;
                if self.drag_project(mouse.row) {
                    return;
                }
                if self.drag_flow(mouse.column, mouse.row) {
                    return;
                }
                if let Some(selection) =
                    self.selection.as_mut().filter(|_| self.selecting.is_some())
                {
                    selection.extend_to(at);
                    self.selecting = Some(at);
                }
                return;
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.release_project() {
                    self.hover = self.hover_target(mouse.column, mouse.row);
                    return;
                }
                if self.release_flow() {
                    return;
                }
                if self.selecting.take().is_some() {
                    if let Some(selection) = self.selection.as_mut() {
                        selection.extend_to(at);
                    }
                    if self.selection.as_ref().is_some_and(|s| !s.is_empty()) {
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
        if matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) && !self.wheel_keeps_selection(panel)
        {
            self.clear_selection();
        }
        match mouse.kind {
            MouseEventKind::ScrollUp if over_detail => self.scroll_detail_up(step),
            MouseEventKind::ScrollDown if over_detail => self.scroll_detail_down(step),
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight
                if panel == PanelFocus::Chat && self.flow.is_open() =>
            {
                self.clear_selection();
                self.scroll_flow(mouse.kind, mouse.modifiers, step);
            }
            MouseEventKind::ScrollUp => self.scroll_panel_up(panel, step),
            MouseEventKind::ScrollDown => self.scroll_panel_down(panel, step),
            MouseEventKind::Down(MouseButton::Left) => {
                self.focus = panel;
                if panel == PanelFocus::Chat && self.open_image_at(mouse.column, mouse.row) {
                    self.clear_selection();
                    self.hover = None;
                    return;
                }
                self.selection_scrolls =
                    panel == PanelFocus::Chat && self.areas.chat_rows.is_some();
                let top = if self.selection_scrolls {
                    self.areas.chat_rows_top
                } else {
                    0
                };
                self.selection = self
                    .selectable_area(panel)
                    .map(|bounds| Selection::new(bounds, top, at));
                self.selecting = self.selection.is_some().then_some(at);
                match panel {
                    PanelFocus::Swarm => {
                        if !self.press_project(mouse.column, mouse.row) {
                            self.click_agents_panel(mouse.column, mouse.row);
                        }
                    }
                    PanelFocus::Sandbox => self.click_side_panel(mouse.column, mouse.row),
                    PanelFocus::Records => self.click_records_panel(mouse.column, mouse.row),
                    PanelFocus::Chat => {
                        if self.detail_close_at(mouse.column, mouse.row) {
                            self.close_detail();
                            self.clear_selection();
                        } else if contains(self.areas.chat_expand, mouse.column, mouse.row) {
                            self.toggle_chat_expanded();
                            self.clear_selection();
                        } else if self.click_chat_tabs(mouse.column, mouse.row) {
                            self.clear_selection();
                        } else if self.detail == Some(DetailView::Graph) {
                            self.click_graph(mouse.column, mouse.row);
                        } else if self.click_thread(mouse.column, mouse.row)
                            || (self.detail.is_none()
                                && ((self.log.is_open()
                                    && self.click_log(mouse.column, mouse.row))
                                    || (self.flow.is_open()
                                        && self.click_flow(mouse.column, mouse.row))))
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

    /// A left press on the input box's stop button cancels the running
    /// turn, as Ctrl+C does. Returns whether the press was on it.
    fn click_stop_button(&mut self, mouse: MouseEvent) -> bool {
        let pressed = mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && self.is_busy()
            && contains(self.areas.input_stop, mouse.column, mouse.row);
        if !pressed {
            return false;
        }
        self.cancel();
        self.clear_selection();
        self.hover = None;
        true
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
        if self.detail_close_at(column, row) {
            return Some(HoverTarget::CloseDetail);
        }
        if contains(self.areas.chat_expand, column, row) {
            return Some(HoverTarget::ExpandChat);
        }
        if contains(self.areas.input_stop, column, row) {
            return Some(HoverTarget::StopTurn);
        }
        if self.agents_tab == AgentsTab::Projects {
            if let Some(index) = self.project_row_at(column, row) {
                if self.project_add_at(index, column).is_some() {
                    return Some(HoverTarget::AddToProject(index));
                }
                if let Some((button, _)) = self.agent_button_hit(index, column) {
                    return Some(button.target(index));
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
        if contains(self.areas.record_tabs, column, row) {
            return tab_at(self.areas.record_tabs, column, row, RecordsTab::layout)
                .map(HoverTarget::RecordTab);
        }
        if let Some(index) = self.record_row_at(column, row) {
            return Some(HoverTarget::RecordRow(index));
        }
        if let Some(index) = self.member_row_at(column, row) {
            return Some(HoverTarget::MemberRow(index));
        }
        if let Some(index) = self.inbox_row_at(column, row) {
            return Some(HoverTarget::InboxRow(index));
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
            ProjectListRow::Project { .. }
                | ProjectListRow::EmptyProject { .. }
                | ProjectListRow::Agent { .. }
        )
        .then_some(index)
    }

    /// Key of the project whose header is `row`.
    pub(super) fn header_key(&self, row: ProjectListRow) -> Option<String> {
        match row {
            ProjectListRow::Project { root, .. } => self.project_key(root),
            ProjectListRow::EmptyProject { project } => {
                self.projects.get(project).map(|p| p.id.clone())
            }
            ProjectListRow::Agent { .. } | ProjectListRow::NewProject | ProjectListRow::Gap => None,
        }
    }

    /// Key of the project whose "+" is at `column` on projects-list row
    /// `index`.
    fn project_add_at(&self, index: usize, column: u16) -> Option<String> {
        let list = self.areas.agent_rows?;
        let key = self.header_key(*self.project_rows().get(index)?)?;
        (column + PROJECT_ADD_WIDTH >= list.x + list.width).then_some(key)
    }

    /// The button at `column` on projects-list row `index`, if the row is
    /// an agent's, and the agent's id. Where the buttons sit follows the
    /// row's time, which is formatted against the clock just as the
    /// renderer does it, and the code column.
    fn agent_button_hit(&self, index: usize, column: u16) -> Option<(AgentButton, String)> {
        let list = self.areas.agent_rows?;
        let ProjectListRow::Agent { index: agent } = *self.project_rows().get(index)? else {
            return None;
        };
        let agent = self.agents.get(agent)?;
        let now = crate::renderer::now_ms();
        let ago = agent
            .last_active_ms
            .map(|ms| crate::renderer::format_ago(ms, now));
        let end = AgentRowEnd::agent(ago.as_deref());
        let offset = column.checked_sub(list.x)?;
        let button = agent_button_at(list.width, &end, offset)?;
        Some((button, agent.id.clone()))
    }

    /// Unfold the project with key `key` and ask for a new agent in it.
    pub(super) fn add_agent_to_project(&mut self, key: &str) {
        self.collapsed_projects.remove(key);
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::AddAgentToProject(key.to_string()));
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
    /// only the rows its body scrolls through, or its message area. The
    /// agents panel on the left is navigation, so its text is never
    /// selected.
    fn selectable_area(&self, panel: PanelFocus) -> Option<Rect> {
        let area = match panel {
            PanelFocus::Chat if self.areas.chat_rows.is_some() => return self.areas.chat_rows,
            PanelFocus::Chat if self.detail.is_some() => self.areas.chat,
            PanelFocus::Chat => return self.areas.chat_text.or(self.areas.chat),
            PanelFocus::Swarm => return None,
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
        self.selecting = None;
        self.selection_scrolled = None;
        self.copy_requested = false;
    }

    /// Whether the selection is in the rows the chat panel's body scrolls
    /// through, as drawn in the last frame.
    fn selection_in_chat_rows(&self) -> bool {
        self.selection_scrolls
            && self
                .selection
                .as_ref()
                .is_some_and(|s| self.areas.chat_rows == Some(s.bounds()))
    }

    /// Whether the wheel over `panel` scrolls a selection along with its
    /// text rather than dropping it.
    fn wheel_keeps_selection(&self, panel: PanelFocus) -> bool {
        panel == PanelFocus::Chat && self.selection_in_chat_rows()
    }

    /// Fit the selection to the frame just drawn into `buffer`: follow the
    /// chat body's scroll, move the held end to the pointer, and keep the
    /// rows in view for copying once they scroll away.
    pub(crate) fn track_selection(&mut self, buffer: &Buffer) {
        if !self.selection_in_chat_rows() {
            return;
        }
        let Some(selection) = self.selection.as_mut() else {
            return;
        };
        selection.scroll_to(self.areas.chat_rows_top);
        if let Some(at) = self.selecting {
            selection.extend_to(at);
        }
        selection.keep(buffer);
    }

    /// How the chat should scroll for a selection being dragged past its
    /// top (`true`) or bottom, and by how many rows.
    fn selection_scroll(&self) -> Option<(bool, usize)> {
        let at = self.selecting?;
        let bounds = self.selection.as_ref()?.bounds();
        if !self.selection_in_chat_rows() {
            return None;
        }
        let (up, past) = if at.y < bounds.top() {
            (true, bounds.top() - at.y)
        } else if at.y >= bounds.bottom() {
            (false, at.y + 1 - bounds.bottom())
        } else {
            return None;
        };
        Some((up, usize::from(past.min(SELECTION_SCROLL_MOST))))
    }

    /// Whether a selection is held past the chat's top or bottom, so the
    /// chat keeps scrolling under it while time passes.
    #[must_use]
    pub fn selection_scrolling(&self) -> bool {
        self.selection_scroll().is_some()
    }

    /// Scroll the chat one step under a selection dragged past its top or
    /// bottom. The selection's held end follows on the next frame.
    pub(crate) fn scroll_selection(&mut self) {
        let Some((up, rows)) = self.selection_scroll() else {
            return;
        };
        match (self.detail.is_some(), up) {
            (true, true) => self.scroll_detail_up(rows),
            (true, false) => self.scroll_detail_down(rows),
            (false, true) => self.scroll_panel_up(PanelFocus::Chat, rows),
            (false, false) => self.scroll_panel_down(PanelFocus::Chat, rows),
        }
    }

    /// Whether a copy is waiting for the next frame to be drawn.
    #[must_use]
    pub const fn copy_pending(&self) -> bool {
        self.copy_requested || self.copy_pending_text.is_some()
    }

    /// Ask for the current selection to be copied after the next frame.
    /// Returns `false` when nothing is selected.
    pub fn request_copy(&mut self) -> bool {
        self.copy_requested = self.selection.as_ref().is_some_and(|s| !s.is_empty());
        self.copy_requested
    }

    /// The text to copy once per copy request: text asked for outright,
    /// else the selected text in `buffer` (the frame just drawn).
    pub fn take_copy_text(&mut self, buffer: &Buffer) -> Option<String> {
        let selection = std::mem::take(&mut self.copy_requested);
        if let Some(text) = self.copy_pending_text.take() {
            return Some(text);
        }
        if !selection {
            return None;
        }
        self.copy_label = None;
        let text = self.selection.as_ref()?.text(buffer);
        (!text.is_empty()).then_some(text)
    }

    /// Report the outcome of copying `chars` characters to the clipboard.
    ///
    pub fn notify_copied(&mut self, chars: usize, result: &std::io::Result<()>) {
        self.redraw = true;
        let what = self.copy_label.take();
        self.log.copy_finished(result.is_ok());
        self.notification = Some(match result {
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

    /// Clicking a project folds or opens it and leaves the highlight where
    /// it was, as does clicking the fold marker of an agent with spawns;
    /// clicking an agent elsewhere or "+ New project" highlights and acts
    /// on it.
    fn click_agent_row(&mut self, column: u16, row: u16) {
        let Some(index) = self.project_row_at(column, row) else {
            return;
        };
        if let Some(key) = self.project_add_at(index, column) {
            self.add_agent_to_project(&key);
        } else if let Some((button, id)) = self.agent_button_hit(index, column) {
            match button {
                AgentButton::Archive => self.archive_agent(id),
            }
        } else if let Some(agent) = self.fold_marker_at(index, column) {
            // Like a project's header, the marker leaves a list the wheel
            // scrolled where it is.
            self.toggle_agent(agent);
        } else if let Some(ProjectListRow::Project { root, .. }) = self.project_rows().get(index) {
            self.toggle_project(*root);
        } else if let Some(ProjectListRow::EmptyProject { .. }) = self.project_rows().get(index) {
            // Nothing to fold or open.
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

    /// Check if the Record panel is visible. It hides while the side
    /// panel is maximized over it.
    #[must_use]
    pub const fn record_panel_visible(&self) -> bool {
        self.record_panel_visible && !self.chat_expanded && !self.side_maximized()
    }

    /// Whether the side panel, shown, takes the Record panel's room too.
    const fn side_maximized(&self) -> bool {
        self.side_expanded && self.sandbox_panel_visible
    }

    /// Check if the Swarm panel is visible.
    #[must_use]
    pub const fn swarm_panel_visible(&self) -> bool {
        self.swarm_panel_visible && !self.chat_expanded
    }

    /// Check if the Sandbox panel is visible.
    #[must_use]
    pub const fn sandbox_panel_visible(&self) -> bool {
        self.sandbox_panel_visible && !self.chat_expanded
    }

    /// Whether the chat panel fills the screen, the other panels hidden.
    #[must_use]
    pub const fn chat_expanded(&self) -> bool {
        self.chat_expanded
    }

    /// Expand the chat panel to fill the screen, or give the other panels
    /// their room back. The chat keeps whichever tab or view it shows.
    pub fn toggle_chat_expanded(&mut self) {
        self.chat_expanded = !self.chat_expanded;
        if self.chat_expanded {
            self.focus = PanelFocus::Chat;
        }
    }

    /// Whether the side panel fills the right column, the Record panel
    /// above it hidden.
    #[must_use]
    pub const fn side_expanded(&self) -> bool {
        self.side_expanded
    }

    /// Maximize the side panel over the Record panel, focused, or put it
    /// back below it with the split it had.
    pub fn toggle_side_expanded(&mut self) {
        self.side_expanded = !self.side_expanded;
        if self.side_expanded {
            self.focus = PanelFocus::Sandbox;
        }
    }

    /// Record where the side panel's expand button was drawn this frame.
    pub fn set_side_expand_area(&mut self, area: Option<Rect>) {
        self.areas.side_expand = area;
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
        let gap = |rows: &mut Vec<ProjectListRow>| {
            if !rows.is_empty() {
                rows.push(ProjectListRow::Gap);
            }
        };
        let mut empty = empty_projects(&self.projects, &self.agents)
            .into_iter()
            .peekable();
        let mut rows = Vec::new();
        let mut index = 0;
        while index < self.agents.len() {
            if starts_project(&self.agents, index) {
                while let Some((_, project)) = empty.next_if(|&(before, _)| before <= index) {
                    gap(&mut rows);
                    rows.push(ProjectListRow::EmptyProject { project });
                }
                let root = index;
                gap(&mut rows);
                let end = self.project_end(root);
                let expanded = !self
                    .collapsed_projects
                    .contains(self.agents[root].project_key());
                rows.push(ProjectListRow::Project { root, expanded });
                if expanded {
                    let mut agent_index = root;
                    while agent_index < end {
                        rows.push(ProjectListRow::Agent { index: agent_index });
                        agent_index = self.next_shown_agent(agent_index);
                    }
                }
                index = end;
            } else {
                rows.push(ProjectListRow::Agent { index });
                index = self.next_shown_agent(index);
            }
        }
        for (_, project) in empty {
            gap(&mut rows);
            rows.push(ProjectListRow::EmptyProject { project });
        }
        rows.push(ProjectListRow::NewProject);
        rows
    }

    /// Index just past the agents of the project whose first agent is
    /// `root`.
    pub(super) fn project_end(&self, root: usize) -> usize {
        (root + 1..self.agents.len())
            .find(|&i| starts_project(&self.agents, i))
            .unwrap_or(self.agents.len())
    }

    /// First agent of the project that contains `index`.
    pub(super) fn project_root(&self, index: usize) -> Option<usize> {
        (0..=index).rev().find(|&i| starts_project(&self.agents, i))
    }

    /// Key of the project whose first agent is `root`.
    pub(super) fn project_key(&self, root: usize) -> Option<String> {
        self.agents
            .get(root)
            .map(|agent| agent.project_key().to_string())
    }

    /// First agent of the project with key `key`.
    pub(super) fn project_root_of_key(&self, key: &str) -> Option<usize> {
        (0..self.agents.len())
            .find(|&i| starts_project(&self.agents, i) && self.agents[i].project_key() == key)
    }

    /// Open a folded project, and any folded agent above it, so `index`
    /// has a visible row.
    fn reveal_agent(&mut self, index: usize) {
        if let Some(key) = self
            .project_root(index)
            .and_then(|root| self.project_key(root))
        {
            self.collapsed_projects.remove(&key);
        }
        self.open_ancestors(index);
    }

    /// Open every folded agent that agent `index` is spawned under.
    pub(super) fn open_ancestors(&mut self, index: usize) {
        let ancestors: Vec<usize> =
            std::iter::successors(self.parent_of(index), |&i| self.parent_of(i)).collect();
        for ancestor in ancestors {
            self.collapsed_agents.remove(&self.agents[ancestor].id);
        }
    }

    /// The agent that spawned agent `index`: the nearest one above it in
    /// its project with a smaller depth.
    fn parent_of(&self, index: usize) -> Option<usize> {
        let depth = self.agents.get(index)?.depth;
        if depth == 0 || starts_project(&self.agents, index) {
            return None;
        }
        let root = self.project_root(index)?;
        (root..index).rev().find(|&i| self.agents[i].depth < depth)
    }

    /// Index just past agent `index` and the agents spawned under it. Its
    /// spawns stop at the next agent no deeper than it, or at the next
    /// project.
    pub(crate) fn spawn_end(&self, index: usize) -> usize {
        let Some(depth) = self.agents.get(index).map(|agent| agent.depth) else {
            return self.agents.len();
        };
        (index + 1..self.agents.len())
            .find(|&i| self.agents[i].depth <= depth || starts_project(&self.agents, i))
            .unwrap_or(self.agents.len())
    }

    /// Whether agent `index` has spawned agents listed under it, so it
    /// can fold.
    pub(crate) fn agent_has_spawns(&self, index: usize) -> bool {
        self.spawn_end(index) > index + 1
    }

    /// Whether agent `index` is folded shut over the agents it spawned.
    pub(crate) fn agent_folded(&self, index: usize) -> bool {
        self.agents
            .get(index)
            .is_some_and(|agent| self.collapsed_agents.contains(&agent.id))
            && self.agent_has_spawns(index)
    }

    /// The agent listed after agent `index`, past its spawns when folded.
    fn next_shown_agent(&self, index: usize) -> usize {
        if self.agent_folded(index) {
            self.spawn_end(index)
        } else {
            index + 1
        }
    }

    /// Fold or open agent `index` over the agents it spawned. Folding it
    /// parks the highlight on it when the highlight was on one of the
    /// rows it hides; opening it again while the highlight is parked there
    /// brings the highlight back to the agent shown in the chat if it is
    /// one of the spawns. Otherwise the highlight stays on the same row as
    /// rows above it come and go.
    pub(crate) fn toggle_agent(&mut self, index: usize) {
        if !self.agent_has_spawns(index) {
            return;
        }
        let id = self.agents[index].id.clone();
        let spawns = index + 1..self.spawn_end(index);
        let before = self.project_rows().get(self.selected_agent).copied();
        let opened = self.collapsed_agents.remove(&id);
        if !opened {
            self.collapsed_agents.insert(id);
        }
        let on_agent = before == Some(ProjectListRow::Agent { index });
        let hidden = matches!(before, Some(ProjectListRow::Agent { index: found }) if spawns.contains(&found))
            && self.agent_folded(index);
        let active_row = || {
            spawns
                .clone()
                .find(|&i| self.agents[i].id == self.active_agent_id)
                .and_then(|i| self.row_of_agent(i))
        };
        let kept = if hidden {
            self.row_of_agent(index)
        } else if opened && on_agent {
            active_row().or_else(|| self.row_of_agent(index))
        } else {
            let rows = self.project_rows();
            before.and_then(|before| rows.iter().position(|row| *row == before))
        };
        if let Some(pos) = kept {
            self.selected_agent = pos;
        }
        let len = self.project_rows().len();
        self.selected_agent = self.selected_agent.min(len.saturating_sub(1));
    }

    /// Whether `agent` is thinking: it runs, or it is the agent on screen
    /// and a turn is in flight.
    pub(crate) fn agent_thinking(&self, agent: &crate::events::AgentSummary) -> bool {
        agent.running || (agent.id == self.active_agent_id && self.is_processing())
    }

    /// Whether the row of agent `index` shows the aura ring: the agent is
    /// thinking, or it is folded over a spawn that is.
    pub(crate) fn agent_row_busy(&self, index: usize) -> bool {
        let Some(agent) = self.agents.get(index) else {
            return false;
        };
        let hidden = if self.agent_folded(index) {
            self.agents.get(index + 1..self.spawn_end(index))
        } else {
            None
        };
        self.agent_thinking(agent)
            || hidden.is_some_and(|hidden| hidden.iter().any(|a| self.agent_thinking(a)))
    }

    /// The agent whose row stands for the agent shown in the chat: that
    /// agent when it has a row, else the nearest agent above it folded
    /// over it. `None` when neither is shown, as under a folded project.
    pub(crate) fn shown_active_agent(&self) -> Option<usize> {
        let index = self
            .agents
            .iter()
            .position(|agent| agent.id == self.active_agent_id)?;
        let rows = self.project_rows();
        let shown = |i: usize| rows.contains(&ProjectListRow::Agent { index: i });
        std::iter::successors(Some(index), |&i| self.parent_of(i)).find(|&i| shown(i))
    }

    /// Agent whose fold marker is at `column` on projects-list row `row`.
    /// An agent with spawns draws its marker as `tree_row` lays the icon
    /// column out: after one leading space and the spawn guides,
    /// `AURA_WIDTH + 2` columns per level. An idle agent's marker opens the
    /// icon column, and the blank rest of that column counts too, as it
    /// does for [`processes::on_row_icon`]. A busy agent's ring stands in
    /// for its marker, open or folded, so the ring and the spare column
    /// after it are the button just the same.
    fn fold_marker_at(&self, row: usize, column: u16) -> Option<usize> {
        use crate::animation::AURA_WIDTH;
        let list = self.areas.agent_rows?;
        let ProjectListRow::Agent { index } = *self.project_rows().get(row)? else {
            return None;
        };
        if !self.agent_has_spawns(index) {
            return None;
        }
        let start = 1 + self.agents[index].depth * (AURA_WIDTH + 2);
        let offset = usize::from(column.checked_sub(list.x)?);
        (start..=start + AURA_WIDTH)
            .contains(&offset)
            .then_some(index)
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
        let Some(id) = self.project_key(root) else {
            return;
        };
        let span = root..self.project_end(root);
        let before = self.project_rows().get(self.selected_agent).copied();
        let selected_inside = before.is_some_and(|row| match row {
            ProjectListRow::Project { root: found, .. } => found == root,
            ProjectListRow::Agent { index } => span.contains(&index),
            ProjectListRow::EmptyProject { .. }
            | ProjectListRow::NewProject
            | ProjectListRow::Gap => false,
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

    /// Move the highlight to the active agent, to the folded agent it is
    /// under, or to its project when folded.
    fn select_visible_active(&mut self) {
        if let Some(row) = self
            .shown_active_agent()
            .and_then(|index| self.row_of_agent(index))
        {
            self.selected_agent = row;
            return;
        }
        if let Some(index) = self
            .agents
            .iter()
            .position(|agent| agent.id == self.active_agent_id)
        {
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

    /// Thinking level pinned for every turn (`high`, `off`, ...), `None`
    /// when each model picks its own.
    #[must_use]
    pub fn thinking(&self) -> Option<&str> {
        self.thinking.as_deref()
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
    /// as a shell command. Prompts that ask for a value (login, folder)
    /// take the line over while they are open.
    #[must_use]
    pub const fn input_is_command(&self) -> bool {
        self.terminal_mode
            && !matches!(
                self.state,
                AppState::LoginEmail
                    | AppState::LoginPassword
                    | AppState::LinkCode
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

    /// Model selector state (tab, filter, highlighted row).
    #[must_use]
    pub const fn model_picker(&self) -> &ModelPicker {
        &self.model_picker
    }

    /// Get the current spinner character for animations.
    #[must_use]
    pub fn spinner_char(&self) -> &'static str {
        const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        SPINNER_FRAMES[self.animation_frame % SPINNER_FRAMES.len()]
    }

    /// Frames drawn so far, to step animations by.
    #[must_use]
    pub const fn animation_frame(&self) -> usize {
        self.animation_frame
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
        self.clear_selection();
    }

    /// Close the view over the chat panel.
    pub(super) fn close_detail(&mut self) {
        self.detail = None;
        self.detail_scroll = 0;
        self.graph.behind = false;
        self.clear_selection();
    }

    /// Record where the close button of the view over the chat panel was
    /// drawn this frame.
    pub fn set_detail_close_area(&mut self, area: Option<Rect>) {
        self.areas.detail_close = area;
    }

    /// Record where the chat panel's expand button was drawn this frame.
    pub fn set_chat_expand_area(&mut self, area: Option<Rect>) {
        self.areas.chat_expand = area;
    }

    fn detail_close_at(&self, column: u16, row: u16) -> bool {
        self.detail.is_some() && contains(self.areas.detail_close, column, row)
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

    /// Start the timer of a notification seen for the first time, and drop
    /// one whose time is up.
    fn expire_notification(&mut self) {
        let Some(current) = &self.notification else {
            self.notification_shown = None;
            self.notification_since = None;
            return;
        };
        let now = Instant::now();
        match &self.notification_shown {
            Some((shown, until)) if shown == current => {
                if now >= *until {
                    self.notification = None;
                    self.notification_shown = None;
                    self.notification_since = None;
                }
            }
            _ => {
                let ttl = match current.1 {
                    NotificationType::Error => ERROR_NOTIFICATION_TTL,
                    NotificationType::Success | NotificationType::Warning => NOTIFICATION_TTL,
                };
                self.notification_shown = Some((current.clone(), now + ttl));
                self.notification_since = Some(now);
            }
        }
    }

    /// How long the notification on screen has been showing; zero before
    /// its timer starts.
    #[must_use]
    pub(crate) fn notification_age(&self) -> Duration {
        self.notification_since
            .map_or(Duration::ZERO, |since| since.elapsed())
    }

    /// Keep the notification on screen until it has scrolled for `read`,
    /// and a moment after.
    pub(crate) fn hold_notification_for(&mut self, read: Duration) {
        if let (Some((_, until)), Some(since)) =
            (&mut self.notification_shown, self.notification_since)
        {
            *until = (*until).max(since + read + NOTIFICATION_TAIL);
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

    /// Stop the commands the agent on screen is running. Its turn goes on
    /// with what they printed.
    pub fn stop_command(&mut self) {
        if let Some(tx) = &self.event_tx {
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

    /// Request a switch to another agent. Agents keep working while
    /// another one is shown.
    ///
    /// A listed agent comes on screen at once, without waiting for the
    /// session: a chat already held in memory is shown as it is and the
    /// session is only told the agent is in front (`FocusAgent`); any
    /// other chat is asked for (`SelectAgent`) and waits, blank, for its
    /// history. The event goes out before the local switch so the
    /// requests the switch makes (LOG, GRAPH, BENCH) reach the session
    /// after it. An agent that is not listed, such as one shown in
    /// another tab of the desktop window, only gets the request; the
    /// screen changes once the session answers.
    pub(super) fn request_agent_switch(&mut self, agent_id: String) {
        if agent_id == self.active_agent_id {
            return;
        }
        if !self.agents.iter().any(|agent| agent.id == agent_id) {
            self.try_send_event(UiEvent::SelectAgent(agent_id));
            return;
        }
        let loaded = self.views.get(&agent_id).is_some_and(|view| view.loaded);
        let event = if loaded {
            UiEvent::FocusAgent(agent_id.clone())
        } else {
            UiEvent::SelectAgent(agent_id.clone())
        };
        // A request that never left gets no answer, and a switch waiting
        // for one would drop every later `SetActiveAgent`.
        if !self.try_send_event(event) {
            return;
        }
        if self.switch_sent.is_none() && self.confirmed_agent.is_empty() {
            self.confirmed_agent.clone_from(&self.active_agent_id);
        }
        self.switch_sent = Some(agent_id.clone());
        self.apply_active_agent(&agent_id);
        self.start_chat_loading();
    }

    /// Send `event` to the session. Returns whether it went out.
    fn try_send_event(&self, event: UiEvent) -> bool {
        self.event_tx
            .as_ref()
            .is_some_and(|tx| tx.try_send(event).is_ok())
    }

    /// Process pending updates from the command channel, and scroll the
    /// chat under a selection held past its top or bottom.
    pub fn tick(&mut self) {
        self.animation_frame = self.animation_frame.wrapping_add(1);
        let shown = self.notification.is_some();
        self.expire_notification();
        if !self.selection_scrolling() {
            self.selection_scrolled = None;
        } else if self
            .selection_scrolled
            .is_none_or(|at| at.elapsed() >= SELECTION_SCROLL_EVERY)
        {
            self.scroll_selection();
            self.selection_scrolled = Some(Instant::now());
        }

        self.receive_commands();
        if std::mem::take(&mut self.sandbox_stale) && self.sandbox_panel_visible {
            self.sandbox.refresh();
            self.redraw = true;
        }
        self.refresh_live_log();

        if matches!(
            self.state,
            AppState::Processing | AppState::AwaitingApproval
        ) {
            self.processing_since.get_or_insert_with(Instant::now);
        } else {
            self.processing_since = None;
        }
        if shown || self.animating() {
            self.redraw = true;
        }
    }

    /// Whether the screen may look different since this was last called:
    /// a command changed what it shows, a tick turned a spinner, or
    /// something it shows changed with time. Commands for agents off
    /// screen count only when they change something on it (a
    /// notification, the projects list's marks). The host draws when this
    /// says so, and after input, which this doesn't track.
    pub fn take_redraw(&mut self) -> bool {
        std::mem::take(&mut self.redraw)
    }

    /// Have the next [`Self::take_redraw`] say the screen changed.
    pub fn request_redraw(&mut self) {
        self.redraw = true;
    }

    /// Whether the screen changes from tick to tick: a spinner turns for
    /// an agent, task or process at work, a notification scrolls, a
    /// selection held past the chat's edge scrolls it, or the chat, LOG
    /// or BENCH tab animates.
    #[must_use]
    pub fn animating(&self) -> bool {
        self.is_busy()
            || self.notification.is_some()
            || self.agents.iter().any(|agent| agent.running)
            || self
                .task_list
                .tasks()
                .iter()
                .any(|task| task.state == crate::events::TaskState::Running)
            || self
                .processes
                .iter()
                .any(|process| process.status == crate::events::ProcessStatus::Running)
            || self.selection_scrolling()
            || self.bench_animating()
            || self.log_animating()
            || self.chat_animating()
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
