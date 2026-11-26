//! Application state machine for the terminal UI.
//!
//! Manages the overall application state, conversation history,
//! panels (Chat, Record and Sandbox), and coordinates between input handling and rendering.

mod agent_switch;
mod agent_views;
mod approval;
mod apps;
mod areas;
pub(crate) mod bench_progress;
pub(crate) mod channel_form;
mod chat_tabs;
mod commands;
mod slash;
pub(crate) use slash::plan_ranges;
mod context_menu;
pub mod file_view;
pub mod flow;
pub mod folder_picker;
mod host_tab;
pub(crate) use aura_ui::format;
pub mod graph;
pub mod hud;
pub(crate) use crate::components::image_preview;
pub mod images;
mod inbox;
mod input_editor;
mod input_state;
mod keys;
mod line_edit;
mod list_state;
mod members;
pub mod mentions;
mod model_picker;
mod mouse;
mod notifications;
mod panel_layout;
mod panes;
mod pending;
mod plan_doc;
pub mod plans;
mod policies;
mod processes;
mod project_drag;
mod prose;
pub(crate) mod queue_panel;
pub mod run_log;
pub mod tasks;
pub use aura_ui::sandbox;
mod chat_hits;
mod detail;
mod hit_targets;
mod init;
mod panel_areas;
mod redraw;
mod scroll;
pub(crate) mod scrollbar;
mod selection;
mod selection_state;
mod session;
pub(crate) mod shell_input;
mod shell_prompt;
mod side_scope;
mod slash_menu;
mod smooth;
mod status;
mod switching;
pub mod tab_bar;
mod ticks;
mod tool_view;
pub mod trace_feed;
mod trace_json;
mod turn;
mod view_state;
pub mod vim;

#[cfg(test)]
mod detail_tests;
#[cfg(test)]
mod queue_panel_tests;
#[cfg(test)]
mod tests;

use crate::{
    alerts::AlertSettings,
    components::{quiet_for, Activity, Message, MoreSection, ShellRun, ToolStep},
    events::{
        AgentDefinitionSummary, AgentSummary, ChannelPostView, ModelOptionView, PlanView,
        ProcessSummary, ProjectSummary, RecordSummary, UiCommand, UiEvent,
    },
    renderer::{ChatLineCache, GroupState},
};
use agent_switch::AgentSwitch;
pub use approval::PendingApproval;
pub use apps::{
    AgentsTab, ArchiveRow, BenchAgent, BenchMode, BenchRow, BenchState, FormField, WorkChoice,
    AUTO_CLASSIFIER, BY_DIFFICULTY, DEFAULT_AGAINST, DIFFICULTIES,
};
pub use chat_tabs::ChatTab;
pub(crate) use context_menu::{MenuTarget, SubmenuEntry};
pub use context_menu::{WorkspaceMove, WorkspaceTarget};
pub use detail::DetailView;
use file_view::FileView;
use folder_picker::FolderPicker;
pub(crate) use hit_targets::{
    CodeCopyHit, CopyText, HoverTarget, ImageHit, ProjectListRow, ToggleRow, ToggleTarget,
};
pub use host_tab::{switch_label, switch_rect, SWITCH_GAP};
use hud::Hud;
pub use inbox::{InboxRow, ThreadView};
use input_state::InputState;
use list_state::ListState;
pub use members::{MemberRow, RecordsTab};
pub use model_picker::{ModelPicker, ModelTab, PickerLayout, PickerTarget};
use notifications::Notifications;
pub use panel_areas::PanelAreas;
use panel_areas::{contains, tab_at};
pub(crate) use panel_areas::{follow_selection, InputRows, InputView};
use panel_layout::PanelLayout;
pub use panes::{Pane, PaneChrome, PaneSplits, PaneTab};
use pending::Pending;
pub(crate) use plan_doc::PlanDoc;
pub use processes::SidePanelTab;
pub use prose::{ProseMeasure, ProseRow};
use ratatui::{buffer::Buffer, layout::Rect};
use sandbox::{SandboxEntryKind, SandboxTree};
pub use selection::Selection;
use selection_state::SelectionState;
pub use side_scope::SideScope;
pub use smooth::ChatScroll;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};
use ticks::Ticks;
use tokio::sync::mpsc;

/// Maximum number of messages to keep in history. Every tool call, thinking
/// block and policy line counts as one.
const MAX_MESSAGES: usize = 2000;

/// Terminal width given to commands run before the chat is first drawn.
const DEFAULT_COMMAND_COLUMNS: u16 = 80;

/// Narrowest terminal a command is given.
const MIN_COMMAND_COLUMNS: u16 = 20;

// Maximum number of records to keep in the list, shared with the loader.
use crate::events::MAX_RECORDS;

/// Time between steps of the chat scrolling under a selection dragged
/// past its top or bottom.
const SELECTION_SCROLL_EVERY: Duration = Duration::from_millis(40);

/// Ticks per spinner step when spinners are all that moves: agents off
/// screen, tasks and processes at work don't redraw every frame.
const SPINNER_TICKS: usize = 3;

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
    /// The channel form is open, making or editing a channel
    ChannelForm,
}

/// Main application struct managing UI state.
pub struct App {
    /// Current application state
    pub(super) state: AppState,
    /// Listed projects, in list order
    pub(super) projects: Vec<ProjectSummary>,
    /// Key of the project whose details are shown
    pub(super) project_info: Option<String>,
    /// Conversation messages
    pub(super) messages: VecDeque<Message>,
    /// The input box: its text, cursor, histories, menus, images and
    /// scroll
    pub(super) input: InputState,
    /// Current status message
    pub(super) status: String,
    /// Things waiting on the user or the host: approvals, confirmations
    /// and requests not yet taken
    pub(super) pending: Pending,
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
    /// Event sender (for sending events to kernel)
    pub(super) event_tx: Option<mpsc::Sender<UiEvent>>,
    /// Command receiver (for receiving commands from kernel)
    pub(super) command_rx: Option<mpsc::Receiver<UiCommand>>,
    /// Current streaming message (being built)
    pub(super) streaming_content: String,
    /// The corner notification and its timer
    pub(super) notifications: Notifications,
    /// Which panel has focus
    pub(super) focus: PanelFocus,
    /// Which panels are shown, and which fill the room of the others
    pub(super) panels: PanelLayout,
    /// File tree of the active agent's sandbox
    pub(super) sandbox: SandboxTree,
    /// Animation, redraw and alert bookkeeping kept from tick to tick
    pub(super) ticks: Ticks,
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
    /// How far the POLICIES tab is scrolled
    pub(super) policies_scroll: scroll::InfoScroll,
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
    /// Switching which agent is on screen
    pub(super) switching: AgentSwitch,
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
    /// Where the model selector's rows were drawn, for clicks
    pub(super) model_picker_layout: PickerLayout,
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
    /// `[user@host dir]$ ` shown before the input line at the terminal prompt
    pub(super) shell_prompt: String,
    /// Width of the chat text area in the last frame (0 before the first)
    pub(super) chat_width: u16,
    /// Whether the chat panel's TERMINAL tab is open (`$` opens it): the
    /// panel shows the session's shell commands, and Enter runs the line
    /// as one
    pub(super) terminal_mode: bool,
    /// Images of messages sent to a busy agent, by message text, until
    /// the chat shows the message
    pub(super) queued_images: VecDeque<(String, Vec<std::sync::Arc<image_preview::ImagePreview>>)>,
    /// Image thumbnails in the chat in the last frame
    pub(super) image_hits: Vec<ImageHit>,
    /// Copy buttons of code blocks in the chat in the last frame
    pub(super) code_copy_hits: Vec<CodeCopyHit>,
    /// Thumbnails of the images waiting in the input, in the last frame
    pub(super) pending_image_areas: Vec<(Rect, usize)>,
    /// Absolute cell rect where the detail-view image was drawn in the last
    /// frame; `None` when no image detail view is open or nothing was drawn.
    pub(super) detail_image_rect: Option<Rect>,
    /// Text selected with the mouse, the chat scrollbar's grab and what
    /// waits to be copied
    pub(super) select: SelectionState,
    /// Which list the agents panel shows
    pub(super) agents_tab: AgentsTab,
    /// What each agent app showed, put back when it comes to the front
    apps: apps::AppMemory,
    /// Channels, and the one open in the CHANNELS tab
    pub(super) channels: apps::ChannelState,
    /// Permanent agents, the highlighted row in the roster (one past the
    /// last agent is "+ New agent") and its first visible row
    pub(super) definitions: ListState<AgentDefinitionSummary>,
    /// Archived agents, each followed by the agents under it; the
    /// highlighted one, and the first visible row of the Archive
    pub(super) archived: ListState<AgentSummary>,
    /// Archived agents opened over the agents under them; every other one
    /// with agents under it is folded. Kept while another app is in front.
    pub(super) opened_archived: HashSet<String>,
    /// Folder browser for a new project, while it is open
    pub(super) folder_picker: Option<FolderPicker>,
    /// The new-channel form, while it is open
    pub(super) channel_form: Option<channel_form::ChannelForm>,
    /// Menu a right click opened on the projects list
    pub(super) context_menu: Option<context_menu::ContextMenu>,
    /// The host's workspaces, which the menu offers to move a project to
    pub(super) workspaces: context_menu::Workspaces,
    /// Usage, todos and jobs of the agent on screen
    pub(super) hud: Hud,
    /// Where each agent runs, and the HOST tab
    pub(super) cloud: host_tab::CloudUi,
    /// The queued-messages panel above the input
    pub(super) queue_panel: queue_panel::QueuePanel,
    /// The benchmark progress panel above the input
    pub(super) bench_panel: bench_progress::BenchPanel,
    /// Whether the full todo list is shown above the input
    pub(super) todos_open: bool,
    /// Whether the chat's live to-do checklist lists every item in full
    pub(super) todos_expanded: bool,
    /// Which view the panel below the Record panel shows
    pub(super) side_tab: SidePanelTab,
    /// Whose items the panel's lists show
    pub(super) side_scope: SideScope,
    /// Every process, of every agent
    pub(super) all_processes: Vec<ProcessSummary>,
    /// Agents whose processes the Processes tab lists
    pub(super) process_filter: side_scope::AgentFilter,
    /// Rows of the Processes tab: the processes `process_filter` lets
    /// through; the highlighted row, and the first visible row
    pub(super) processes: ListState<ProcessSummary>,
    /// Run graph shown by `/graph`
    pub(super) graph: graph::GraphState,
    pub(super) trace_feed: trace_feed::TraceFeed,
    /// First visible row of the TRACE tab
    pub(super) trace_scroll: usize,
    /// Plans and tasks of the TASKS tab
    pub(super) task_list: tasks::TaskList,
    /// First visible row of the TASKS tab
    pub(super) tasks_scroll: usize,
    /// Flow between agents shown by the chat panel's GRAPH tab
    pub(super) flow: flow::FlowState,
    /// Benchmarks, and the one shown in the BENCH tab
    pub(super) bench: BenchState,
    /// Run log shown by the chat panel's LOG tab
    pub(super) log: run_log::LogState,
}

/// Columns of the " + " at the right end of a project's row, which adds an
/// agent to the project when clicked.
pub(crate) const PROJECT_ADD_WIDTH: u16 = 3;

pub(crate) use crate::components::{agent_button_at, AgentButton, AgentRowEnd};

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
