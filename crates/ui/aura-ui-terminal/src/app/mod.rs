//! Application state machine for the terminal UI.
//!
//! Manages the overall application state, conversation history,
//! panels (Chat, Record and Sandbox), and coordinates between input handling and rendering.

mod account;
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
mod host_chrome;
mod host_tab;
pub(crate) use aura_ui::format;
pub mod graph;
pub mod hud;
pub(crate) use crate::components::image_preview;
pub mod images;
mod inbox;
mod input_editor;
mod input_state;
mod kernel_link;
mod keys;
mod line_edit;
mod list_state;
mod members;
pub mod mentions;
mod modal;
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
mod project_list;
mod prose;
pub(crate) mod queue_panel;
mod record_list;
mod remote_files;
pub(crate) mod repo_tab;
pub mod run_log;
pub mod tasks;
pub use aura_ui::sandbox;
mod chat_hits;
pub(crate) mod chat_links;
mod chat_view;
mod detail;
mod detail_state;
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
mod side_panel;
mod side_scope;
mod slash_menu;
mod smooth;
mod status;
mod switching;
pub mod tab_bar;
mod ticks;
pub(crate) mod todo_drawer;
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
        ProcessSummary, RecordSummary, UiCommand, UiEvent,
    },
    renderer::{ChatLineCache, GroupState},
};
use account::Account;
use agent_switch::AgentSwitch;
pub use approval::PendingApproval;
pub(crate) use apps::CHANNEL_MEMBER_ROWS;
pub use apps::{
    AgentsTab, ArchiveRow, BenchAgent, BenchMode, BenchRow, BenchState, FormField, WorkChoice,
    AUTO_CLASSIFIER, BY_DIFFICULTY, DEFAULT_AGAINST, DIFFICULTIES,
};
pub(crate) use chat_links::is_openable_url;
pub use chat_tabs::ChatTab;
use chat_view::ChatView;
pub(crate) use context_menu::{MenuTarget, SubmenuEntry};
pub use context_menu::{WorkspaceMove, WorkspaceTarget};
pub use detail::DetailView;
use detail_state::DetailState;
use file_view::FileView;
pub(crate) use hit_targets::{
    CodeCopyHit, CopyText, HoverTarget, ImageHit, LinkHit, ProjectListRow, ToggleRow, ToggleTarget,
};
use host_chrome::HostChrome;
pub(crate) use host_tab::switch_button;
pub use host_tab::{switch_label, switch_rect, SWITCH_GAP};
use hud::Hud;
pub use inbox::{InboxRow, ThreadView};
use input_state::InputState;
use kernel_link::KernelLink;
use list_state::ListState;
pub use members::{MemberRow, RecordsTab};
pub use modal::{Dialog, ModalKind};
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
use project_list::ProjectList;
pub use prose::{ProseMeasure, ProseRow};
use ratatui::{buffer::Buffer, layout::Rect};
use record_list::RecordList;
use sandbox::{SandboxEntryKind, SandboxTree};
pub use selection::Selection;
use selection_state::SelectionState;
use side_panel::SidePanel;
pub use side_scope::SideScope;
pub use smooth::ChatScroll;
use std::collections::VecDeque;
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
    /// What the chat is doing: idle, at work, waiting on an approval, or
    /// asking for a sign-in on the input line. A dialog open over it is
    /// kept apart, in `modals`, so a turn moving on leaves the dialog be.
    pub(super) chat_state: AppState,
    /// The dialog open over the screen, under an approval or not, where
    /// the modal on top was drawn, and the help dialog's scroll
    pub(super) modals: modal::ModalLayer,
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
    /// The chat as last drawn: rendered lines, rows, images and buttons
    pub(super) chat: ChatView,
    /// The channels to and from the kernel
    pub(super) link: KernelLink,
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
    /// Kernel records list, its selection, scroll and tab
    pub(super) records: RecordList,
    /// The agents of the run of the agent on screen, for the MEMBERS tab
    pub(super) members: members::Members,
    /// The threads of mail between agents, for the INBOX tab
    pub(super) inbox: inbox::Inbox,
    /// Record or file shown over the chat panel, and its scroll
    pub(super) detail: DetailState,
    /// List of agents in the swarm
    pub(super) agents: Vec<AgentSummary>,
    /// The projects list: projects, highlight, scroll, folds and drags
    pub(super) project_list: ProjectList,
    /// Currently active agent ID
    pub(super) active_agent_id: String,
    /// Switching which agent is on screen
    pub(super) switching: AgentSwitch,
    /// The signed-in user, plans and models
    pub(super) account: Account,
    /// Screen areas from the last frame, for mouse hit-testing
    pub(super) areas: PanelAreas,
    /// What the front end draws or decides itself
    pub(super) host: HostChrome,
    /// Clickable item under the mouse pointer
    hover: Option<HoverTarget>,
    /// `[user@host dir]$ ` shown before the input line at the terminal prompt
    pub(super) shell_prompt: String,
    /// Whether the chat panel's TERMINAL tab is open (`$` opens it): the
    /// panel shows the session's shell commands, and Enter runs the line
    /// as one
    pub(super) terminal_mode: bool,
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
    /// The new-channel form, while it is open
    pub(super) channel_form: Option<channel_form::ChannelForm>,
    /// Menu a right click opened on the projects list
    pub(super) context_menu: Option<context_menu::ContextMenu>,
    /// Usage, todos and jobs of the agent on screen
    pub(super) hud: Hud,
    /// The queued-messages panel above the input
    pub(super) queue_panel: queue_panel::QueuePanel,
    /// The benchmark progress panel above the input
    pub(super) bench_panel: bench_progress::BenchPanel,
    /// The to-do drawer under the Chat body
    pub(super) todo_drawer: todo_drawer::TodoDrawer,
    /// The panel below the Record panel: its tab, scope, processes and
    /// scroll positions
    pub(super) side: SidePanel,
    /// Run graph shown by `/graph`
    pub(super) graph: graph::GraphState,
    pub(super) trace_feed: trace_feed::TraceFeed,
    /// Plans and tasks of the TASKS tab
    pub(super) task_list: tasks::TaskList,
    /// The REPO tab's view, filter and rows
    pub(super) repo: repo_tab::RepoTab,
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
