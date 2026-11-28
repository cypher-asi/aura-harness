//! Events the UI sends to the application logic.

use super::{BenchRequest, ImageAttachment, LogRequest};
#[cfg(doc)]
use super::{ProcessSummary, QueuedItemView, UiCommand};
use crate::mode_command::{ApprovalLevel, PermissionMode};
use std::path::PathBuf;

/// Events sent from the UI to the application logic.
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// User submitted a message to an agent
    UserMessage {
        /// Agent the chat showed (hex id)
        agent_id: String,
        /// Message text
        text: String,
        /// Whether the chat already shows the message. It doesn't when the
        /// agent was busy and the message waits for its turn.
        shown: bool,
        /// Images pasted into the message; `text` holds an `[image N]`
        /// chip for each
        images: Vec<ImageAttachment>,
    },

    /// User typed a shell command to run in the agent's workspace
    RunCommand {
        /// Agent the chat showed (hex id)
        agent_id: String,
        /// Command line
        command: String,
        /// Width of the terminal the command's output is shown in
        cols: u16,
    },

    /// User approved a pending tool request
    Approve(String),

    /// User approved a pending tool request and wants calls like it to run
    /// without asking in this project from now on
    ApproveAlways(String),

    /// User denied a pending tool request
    Deny(String),

    /// User picked the work mode of an agent (`/mode <name>`)
    SetMode {
        /// Agent the chat showed (hex id)
        agent_id: String,
        /// Mode for the agent's next tool calls
        mode: PermissionMode,
    },

    /// User picked the approval level of an agent (`/approval <level>`)
    SetApproval {
        /// Agent the chat showed (hex id)
        agent_id: String,
        /// Level for the agent's next tool calls
        level: ApprovalLevel,
    },

    /// User pressed Shift+Tab to move the agent with this hex id to the
    /// next approval level
    CycleApproval(String),

    /// User ran `/plan` for the agent with this hex id. Enters plan mode,
    /// or leaves it and returns to the mode from before when already there.
    TogglePlan(String),

    /// User requested to quit
    Quit,

    /// User requested status display
    ShowStatus,

    /// User asked what Gate has learned and how it routed lately (`/gate`)
    ShowGate,

    /// User requested help
    ShowHelp,

    /// User requested history (with optional count)
    ShowHistory(Option<usize>),

    /// User requested to clear the screen
    Clear,

    /// User cancelled the turn of the agent with this hex id
    Cancel(String),

    /// User asked to send the oldest queued message of the agent with this
    /// hex id now: its running turn is cancelled and the message runs next,
    /// ahead of the rest of the queue.
    SendQueuedNow(String),

    /// User removed one queued message or command from an agent's queue
    RemoveQueued {
        /// Agent the queue belongs to (hex id)
        agent_id: String,
        /// [`QueuedItemView::id`] of the entry
        id: u64,
    },

    /// User edited the text of a queued message. Entries that aren't
    /// [`QueuedItemView::editable`] are left as they are.
    EditQueued {
        /// Agent the queue belongs to (hex id)
        agent_id: String,
        /// [`QueuedItemView::id`] of the entry
        id: u64,
        /// The message's new text
        text: String,
    },

    /// User moved a queued entry to another place in the queue
    MoveQueued {
        /// Agent the queue belongs to (hex id)
        agent_id: String,
        /// [`QueuedItemView::id`] of the entry
        id: u64,
        /// Index the entry moves to, oldest first; clamped to the queue
        to: usize,
    },

    /// User asked to run one queued entry now: it moves to the front of
    /// the queue and runs as with [`Self::SendQueuedNow`].
    SendQueuedItemNow {
        /// Agent the queue belongs to (hex id)
        agent_id: String,
        /// [`QueuedItemView::id`] of the entry
        id: u64,
    },

    /// User stopped the commands the agent with this hex id is running;
    /// its turn goes on with what they printed
    StopCommand(String),

    /// User requested a new session (reset context)
    NewSession,

    /// User selected a different agent
    SelectAgent(String),

    /// A view that already holds the chat of the agent with this hex id
    /// came to the front (a tab of the desktop window). The agent becomes
    /// the active one, as with [`Self::SelectAgent`], but its chat is not
    /// sent again.
    FocusAgent(String),

    /// User requested the agent list
    RefreshAgents,

    /// User requested a new agent (with optional display name)
    CreateAgent(Option<String>),

    /// User picked a folder for a project. A new project, named after the
    /// folder, gets a default agent working in it; a folder that is a
    /// project already opens that project. With `switch`, the screen moves
    /// to that agent. The channel form leaves it false, stays open and
    /// selects the project once it is listed.
    CreateProject {
        /// Folder the project's agents work in
        folder: PathBuf,
        /// Whether to show the project's agent. The projects list does;
        /// the channel form does not.
        switch: bool,
    },

    /// User asked for a new agent in the project with this id
    AddAgentToProject(String),

    /// User dragged projects into this order, by id. Projects it doesn't
    /// name keep their places.
    ReorderProjects(Vec<String>),

    /// User dragged the top-level agents of a project into this order
    ReorderAgents {
        /// Project id
        project_id: String,
        /// Hex ids of its top-level agents, in their new order
        order: Vec<String>,
    },

    /// User moved the agent with this hex id, and the agents under it, to
    /// the Archive
    ArchiveAgent(String),

    /// User brought the agent with this hex id back from the Archive
    RestoreAgent(String),

    /// User confirmed removing the project with this id from the projects
    /// list
    RemoveProject(String),

    /// User started an instance of the named permanent agent
    InstantiateDefinition(String),

    /// User added an instance of the named permanent agent to the project
    /// that contains the active agent
    AddDefinitionToProject(String),

    /// User asked to edit the named permanent agent, or to create one (`None`)
    EditDefinition(Option<String>),

    /// User confirmed deleting the named permanent agent
    DeleteDefinition(String),

    /// The editor opened by [`UiCommand::OpenEditor`] exited
    DefinitionEdited(PathBuf),

    /// User submitted login credentials from the TUI
    LoginCredentials {
        /// Email address
        email: String,
        /// Password
        password: String,
    },

    /// User requested zOS logout
    Logout,

    /// User asked to link a personal plan (`claude`, `chatgpt`). With
    /// `paste`, the provider shows a code to paste instead of
    /// redirecting to a local callback.
    LinkSubscription {
        /// Provider id
        provider: String,
        /// Use the paste-code flow
        paste: bool,
    },

    /// User pasted the code for the pending plan link
    LinkCode(String),

    /// User asked to unlink a personal plan
    Unlink(String),

    /// User requested auth status
    Whoami,

    /// User selected a model by id
    SelectModel(String),

    /// User pinned the thinking level of every turn (`high`, `off`, ...);
    /// `None` lets each model pick its own
    SetThinking(Option<String>),

    /// User confirmed killing the process with this [`ProcessSummary::id`]
    KillProcess(String),

    /// User asked for the run graph: the active conversation's run
    /// (`None`) or a recorded run by id or prefix
    OpenGraph(Option<String>),

    /// User opened the GRAPH tab for the agent with this hex id: send its
    /// flow, and keep it up to date
    OpenFlow(String),

    /// User opened the Kernel panel's POLICIES tab for the agent with this
    /// hex id: send the policy its kernel enforces now
    RequestPolicies(String),

    /// The Sandbox panel, showing the active agent's machine
    /// ([`UiCommand::SetSandboxRemote`]), needs this directory listed:
    /// answer with [`UiCommand::SandboxListing`]
    ListSandbox(PathBuf),

    /// User opened this file of the active agent's machine: answer with
    /// [`UiCommand::SandboxFile`]
    ReadSandboxFile(PathBuf),

    /// User labeled a recorded step (`seq`) or the whole run (`None`);
    /// positive is good
    LabelStep {
        run_id: String,
        seq: Option<u64>,
        value: f64,
    },

    /// User asked for the list of recorded runs
    ListRuns,
    /// User asked the agent with this hex id to build a plan: the given
    /// workspace-relative path, or the agent's latest plan
    BuildPlan {
        /// Agent the chat showed (hex id)
        agent_id: String,
        /// Plan file; `None` builds the agent's latest plan
        path: Option<String>,
    },

    /// User asked for the agent's earlier sessions (`None`), or to resume
    /// the one with this number in that list
    Resume {
        /// Agent the chat showed (hex id)
        agent_id: String,
        /// 1-based position in the list
        index: Option<usize>,
    },

    /// User asked to summarize the older history of an agent now
    Compact {
        /// Agent the chat showed (hex id)
        agent_id: String,
        /// What the summary should keep, in the user's words
        focus: Option<String>,
    },

    /// User asked an agent to write (or improve) its project's `AGENTS.md`
    InitProject {
        /// Agent the chat showed (hex id)
        agent_id: String,
    },

    /// User asked for the agent's turns (`turn: None`), or to rewind to
    /// before the turn with this number in that list
    Rewind {
        /// Agent the chat showed (hex id)
        agent_id: String,
        /// Turn number, as `/rewind` lists it
        turn: Option<usize>,
        /// What to put back
        scope: RewindScope,
    },

    /// User asked what the agent's latest turn changed in files, or the
    /// whole session with `session`
    Diff {
        /// Agent the chat showed (hex id)
        agent_id: String,
        /// Every change since the session started
        session: bool,
    },

    /// User ran `/compare` (Gate against another model on their own task)
    /// with these arguments, as typed
    Compare {
        /// Agent the chat showed (hex id)
        agent_id: String,
        /// Everything after `/compare`
        args: String,
    },

    /// The BENCH tab asked for something
    Bench {
        /// Agent it asks for (hex id): the one a new benchmark starts
        /// from, or whose chat shows a run's diff
        agent_id: String,
        request: BenchRequest,
    },

    /// The LOG tab asked for something
    Log {
        /// Agent the chat showed (hex id)
        agent_id: String,
        request: LogRequest,
    },

    /// The REPO tab asked for something
    Repo {
        /// Agent whose repo the tab shows (hex id)
        agent_id: String,
        request: crate::repo::RepoRequest,
    },

    /// Move an agent between this machine and its own cloud machine (the
    /// footer switch, `/cloud`, `/local`)
    SetPlacement {
        /// Agent the chat showed (hex id)
        agent_id: String,
        placement: crate::host::PlacementView,
    },

    /// A HOST tab button or `/cloud pull|sleep|wake`
    HostAction {
        /// Agent the chat showed (hex id)
        agent_id: String,
        action: crate::host::HostAction,
    },

    /// `/cloud setup`: put a boxd API key in `AURA_HOME/.env`
    CloudSetup,

    /// User made a channel
    CreateChannel {
        /// Short unique name it is addressed by, such as `release`
        slug: String,
        /// What it is called
        title: String,
        /// What it is for
        description: String,
        /// Project its agents work in (`None`: no project)
        project_id: Option<String>,
        /// Where the members it starts run
        placement: crate::host::PlacementView,
    },

    /// User opened a channel (channel id)
    OpenChannel(String),

    /// User left the channel that was open
    CloseChannel,

    /// User edited a channel: title, description and project
    UpdateChannel {
        /// Channel id
        channel_id: String,
        /// New title
        title: String,
        /// New description
        description: String,
        /// Project its agents work in; the edit form always sends the full
        /// state, so `None` clears the project
        project_id: Option<String>,
        /// Where the members it starts from now on run; members it has
        /// stay where they are
        placement: crate::host::PlacementView,
    },

    /// User archived a channel (channel id)
    ArchiveChannel(String),

    /// User ran `/plan` in a channel's chat. The coordinator doesn't plan:
    /// a new member of the channel writes the plan in plan mode, in a chat
    /// of its own that the session puts on screen.
    PlanInChannel {
        /// Coordinator whose chat it was typed in (hex id)
        agent_id: String,
        /// What to plan; empty leaves the planner waiting for it
        text: String,
        /// Images pasted into the message; `text` holds an `[image N]`
        /// chip for each
        images: Vec<ImageAttachment>,
    },
}

/// What `/rewind` puts back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RewindScope {
    /// Files the agent changed with its file tools
    Code,
    /// The conversation the model sees
    Chat,
    /// Files and conversation
    #[default]
    Both,
}

impl RewindScope {
    /// Whether files are put back.
    #[must_use]
    pub const fn code(self) -> bool {
        matches!(self, Self::Code | Self::Both)
    }

    /// Whether the conversation is cut back.
    #[must_use]
    pub const fn chat(self) -> bool {
        matches!(self, Self::Chat | Self::Both)
    }

    /// `code`, `chat` or `both`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Chat => "chat",
            Self::Both => "both",
        }
    }
}

impl std::str::FromStr for RewindScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "code" | "files" => Ok(Self::Code),
            "chat" | "conversation" => Ok(Self::Chat),
            "both" | "all" => Ok(Self::Both),
            other => Err(format!("unknown rewind scope {other:?}")),
        }
    }
}
