//! UI event types for communication between UI and kernel.
//!
//! The event system uses channels for bidirectional communication:
//! - `UiEvent`: Events from the UI to the kernel (user actions)
//! - `UiCommand`: Commands from the kernel to the UI (state updates)

pub use crate::benchmarks::{
    arm_letter, cost_per_fix, cost_shares, per_fix_cells, run_order, BenchArm, BenchBaseline,
    BenchDoing, BenchExample, BenchJudge, BenchLiveRun, BenchPhase, BenchProgress, BenchRequest,
    BenchRun, BenchSession, BenchSpec, BenchStatus, BenchTaskCell, BenchTaskRow, BenchTaskSetup,
    BenchTokens, BenchTurn, BenchVerdict, BenchView, BenchWork,
};
pub use crate::flow::{FlowBench, FlowKind, FlowLane, FlowNode, FlowUsage};
use crate::mode_command::{ApprovalLevel, PermissionMode};
pub use crate::run_log::{
    LogField, LogRequest, LogRunView, LogSection, LogStatus, LogStepView, LogTurnView, LogView,
};
use serde::{Deserialize, Serialize};
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
    /// project already opens that project.
    CreateProject {
        /// Folder the project's agents work in
        folder: PathBuf,
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
        /// Agent the chat showed (hex id)
        agent_id: String,
        request: BenchRequest,
    },

    /// The LOG tab asked for something
    Log {
        /// Agent the chat showed (hex id)
        agent_id: String,
        request: LogRequest,
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

/// How one file changed, as a unified diff of its hunks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiffData {
    /// File, relative to the agent's workspace when inside it
    pub path: String,
    /// Unified diff hunks (`@@` headers and `+`/`-`/` ` lines), or a note
    /// such as for a binary file
    pub diff: String,
}

/// Commands sent from the application logic to the UI.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum UiCommand {
    /// Set the status message
    SetStatus(String),

    /// Append text to the current streaming message.
    /// This is used for real-time streaming output from the model.
    AppendText(String),

    /// Start a new streaming message (clears any pending streaming content).
    StartStreaming,

    /// Finalize the streaming message and display it.
    FinishStreaming,

    /// The models of the turn so far and the level they thought at, for
    /// the replies shown since the last user message (sent again as each
    /// model call finishes)
    SetReplyModel {
        /// The models the turn ran on and their thinking level, e.g.
        /// `claude-sonnet-5-5 ×2 · high thinking · gate: claude-haiku-5-5`.
        /// The level is left out when the turn's calls recorded none.
        model: String,
        /// Tokens and cost of all the turn's calls, when any reported usage
        usage: Option<CallUsage>,
        /// Those tokens split by the sign-in that paid for them, in the
        /// order each was first used
        sources: Vec<SourceUsage>,
    },

    /// Append text to the current thinking block.
    /// This is used for real-time display of extended thinking.
    AppendThinking(String),

    /// Start a new thinking block.
    StartThinking,

    /// Finalize the thinking block.
    FinishThinking,

    /// Show a new message
    ShowMessage(MessageData),

    /// Show a tool execution card
    ShowTool(ToolData),

    /// Replace a running tool's arguments (they stream in as partial JSON)
    UpdateToolArgs {
        /// Tool use ID
        id: String,
        /// Arguments as JSON so far
        args: String,
    },

    /// The policy decision on a tool call, e.g. `allowed by user.on`. Shown
    /// with the call; as a `policy: <tool> …` line if the call isn't shown.
    ToolPolicy {
        /// Tool use ID
        id: String,
        /// Tool name
        tool: String,
        /// Verdict and deciding rule
        decision: String,
    },

    /// A tool call failed and is about to be retried
    ToolRetrying {
        /// Tool use ID
        id: String,
        /// 1-based attempt about to run
        attempt: u32,
        /// Total attempts allowed
        max_attempts: u32,
    },

    /// What a file-changing tool call did to its file, as unified diff
    /// hunks, shown in the call's block
    ToolDiff {
        /// Tool use ID
        id: String,
        /// Unified diff hunks
        diff: String,
    },

    /// Show file changes in the chat
    ShowDiff {
        /// What the changes are, e.g. which turn made them
        title: String,
        /// One entry per changed file
        files: Vec<FileDiffData>,
    },

    /// Update a tool's completion status
    CompleteTool {
        /// Tool use ID
        id: String,
        /// Result content
        result: String,
        /// Whether the tool succeeded
        success: bool,
    },

    /// Output of the shell command from the last [`UiEvent::RunCommand`]
    CommandFinished {
        /// Terminal output, possibly with ANSI styling
        output: String,
        /// Exit code when the command failed
        exit_code: Option<i32>,
    },

    /// Request user approval for a tool. The agent's turn waits for the
    /// answer.
    RequestApproval {
        /// Approval request ID, sent back with the answer
        id: String,
        /// Tool name
        tool: String,
        /// Description of the action
        description: String,
        /// What "always allow" would allow from now on, e.g. "`cargo test`
        /// commands in this project"
        always: String,
    },

    /// The approval with this ID stopped waiting (it timed out or the turn
    /// was cancelled)
    CloseApproval(String),

    /// The agent's work mode
    SetMode(PermissionMode),

    /// The agent's approval level
    SetApproval(ApprovalLevel),

    /// Show an error notification
    ShowError(String),

    /// Show a success notification
    ShowSuccess(String),

    /// Show a warning notification
    ShowWarning(String),

    /// Mark the current operation as complete
    Complete,

    /// The agent started a turn the chat didn't start itself (a queued
    /// message, or a task from the agent that spawned it)
    TurnStarted {
        /// Message to show as the user's, when the chat doesn't show it yet
        prompt: Option<String>,
    },

    /// A message the user sent to a busy agent went into its running turn
    UserSteered(String),

    /// Usage of one model call, added to the agent's session totals
    CallStats(CallStats),
    /// Older history was summarized to free context; the text says how much
    Compacted(String),
    /// The agent's todo list
    SetTodos(Vec<TodoView>),
    /// The agent's background jobs
    SetJobs(Vec<JobView>),
    /// The messages and commands waiting for the agent's current work,
    /// in the order they will run (oldest first)
    SetQueue(Vec<QueuedItemView>),
    /// What the agent's context is made of, as of its last turn
    SetContext(ContextView),
    /// Usage of the agent's session so far, when it is opened or resumed
    SetSessionTotals(SessionTotals),
    /// The project instruction files the agent's turns load
    SetInstructions(Vec<InstructionFileView>),
    /// The policy the agent's kernel enforces now, for the POLICIES tab
    SetPolicies(PolicyView),
    /// A message one agent sent another, for the INBOX tab. Global like
    /// [`Self::SetProcesses`]: the UI files it under its thread and shows
    /// it while either agent is in the run on screen.
    AddMail(MailView),

    /// A command for the chat of one agent. Agents work in parallel, so
    /// anything an agent's turn sends is addressed to it; the UI applies it
    /// to that agent's chat whether or not it is on screen.
    ForAgent {
        /// Agent ID (hex encoded)
        agent_id: String,
        /// Command for that agent's chat
        command: Box<UiCommand>,
    },

    /// Clear the conversation
    ClearConversation,

    /// A new record was added to the kernel
    NewRecord(RecordSummary),

    /// Update the listed projects, in list order. Sent before the
    /// [`Self::SetAgents`] that lists their agents.
    SetProjects(Vec<ProjectSummary>),

    /// Update the list of agents in the swarm, project by project
    SetAgents(Vec<AgentSummary>),

    /// Update the Archive: archived agents, each followed by the agents
    /// under it. `depth` counts from the archived agent and `project`
    /// names the project it came from.
    SetArchivedAgents(Vec<AgentSummary>),

    /// Set the currently active agent
    SetActiveAgent(String),

    /// The session could not make the agent with this hex id the active
    /// one (its session failed to open). A UI that already switched to it
    /// on screen goes back to the agent the session last confirmed, so it
    /// never waits for a [`Self::SetActiveAgent`] that will not come.
    SwitchFailed(String),

    /// Sent inside [`Self::ForAgent`] instead of a
    /// [`Self::LoadConversation`] when the session leaves the agent's chat
    /// as it is: the agent is running, so the chat the UI holds is live
    /// and a reload would drop what is streaming. It ends the chat's
    /// loading state.
    ChatReady,

    /// Update the list of permanent agents
    SetAgentDefinitions(Vec<AgentDefinitionSummary>),

    /// Suspend the UI and open a file in the user's editor
    OpenEditor(PathBuf),

    /// Clear records (when switching agents)
    ClearRecords,

    /// Replace the Record panel with these records, oldest first (when
    /// loading history or switching agents). One command instead of a
    /// [`Self::ClearRecords`] and a [`Self::NewRecord`] per record, so a
    /// long history fits in a bounded command channel. Only the newest
    /// [`MAX_RECORDS`] are kept.
    SetRecords(Vec<RecordSummary>),

    /// Replace the conversation with an agent's history (when switching agents)
    LoadConversation(Vec<MessageData>),

    /// Set the sandbox directory shown in the Sandbox panel (the active agent's workspace)
    SetSandboxRoot(PathBuf),

    /// Replace the Processes tab: everything running, plus recently finished work
    SetProcesses(Vec<ProcessSummary>),

    /// Replace the run graph and show it over the chat panel
    GraphLoad {
        /// Run the nodes belong to
        run_id: String,
        /// Nodes in parent-before-child order
        nodes: Vec<TraceNode>,
    },

    /// Add or update nodes of the run graph while the run is live.
    /// Ignored when the graph shows a different run. Sent inside
    /// [`UiCommand::ForAgent`], it also feeds that agent's TRACE tab.
    GraphUpsert {
        /// Run the nodes belong to
        run_id: String,
        /// New or changed nodes, parents before children
        nodes: Vec<TraceNode>,
    },

    /// Replace the flow of the GRAPH tab. Ignored by a UI showing another
    /// agent.
    FlowLoad {
        /// Hex id of the agent the flow was asked for
        agent_id: String,
        /// That agent, then every agent it spawned
        lanes: Vec<FlowLane>,
        /// Nodes, oldest first within each agent
        nodes: Vec<FlowNode>,
    },

    /// Add or update lanes and nodes of the flow as agents work. Ignored
    /// by a UI whose GRAPH tab shows another agent's flow.
    FlowUpsert {
        /// Hex id of the agent the flow was asked for
        agent_id: String,
        /// Agents that joined the flow
        lanes: Vec<FlowLane>,
        /// New nodes and updates to earlier ones ([`FlowNode::merge`])
        nodes: Vec<FlowNode>,
    },

    /// Replace the BENCH tab's list with the benchmarks of the tree
    /// `agent_id` is in, whose top agent is `root`, oldest first. Ignored
    /// by a UI showing another agent.
    BenchList {
        agent_id: String,
        root: String,
        benches: Vec<BenchView>,
    },

    /// A benchmark started or changed; add it to the BENCH tab or replace
    /// it there. Ignored by a UI listing another tree's benchmarks.
    BenchUpsert(Box<BenchView>),

    /// The sessions of `agent_id` the BENCH tab can replay, newest first,
    /// and the built-in example tasks it can run, in the order a
    /// benchmark takes them. Ignored by a UI showing another agent.
    BenchSessions {
        agent_id: String,
        sessions: Vec<BenchSession>,
        examples: Vec<BenchExample>,
    },

    /// What the benchmark the BENCH tab set up for `agent_id` should cost,
    /// waiting to be started; or why it could not be set up
    BenchEstimate {
        agent_id: String,
        estimate: Result<String, String>,
    },

    /// What a BENCH tab action did (pick, apply, dismiss, ...), or why it
    /// could not
    BenchNotice(Result<String, String>),

    /// The LOG tab's content for `view.agent_id`. Ignored by a UI showing
    /// another agent.
    LogView(Box<LogView>),

    /// Why the LOG tab of `agent_id` has nothing to show (no run yet,
    /// tracing off, ...)
    LogNotice { agent_id: String, notice: String },

    /// Set the API status (URL and whether it's active)
    SetApiStatus {
        /// API URL
        url: Option<String>,
        /// Whether the API is active
        active: bool,
    },

    /// Update the model selector contents and the current model
    SetModels {
        /// Selectable models, in display order
        options: Vec<ModelOptionView>,
        /// Id of the selected model (`gate` when Gate picks it)
        current: String,
        /// The model and route Gate sends the next turn to, e.g.
        /// `claude-sonnet-4-6 (claude plan)`; `None` for a pinned model
        route: Option<String>,
        /// Display name of the logged-in user, `None` when browsing as guest
        user: Option<String>,
        /// Labels of the linked plans that can serve requests (e.g. "Claude")
        plans: Vec<String>,
        /// Thinking level pinned for every turn (`high`, `off`, ...);
        /// `None` when each model picks its own
        thinking: Option<String>,
    },

    /// Ask the user to paste the code shown by `provider`'s sign-in page
    PromptLinkCode {
        /// Provider label shown in the prompt
        provider: String,
    },

    /// A plan link attempt finished (successfully or not); drop any model
    /// pick that was waiting on it
    EndLink,

    /// The active agent's current plan, `None` when it has none
    SetPlan(Option<PlanView>),

    /// Every plan in the active agent's workspace, newest first
    SetPlans(Vec<PlanView>),

    /// Every task in the active agent's workspace, newest first
    SetTasks(Vec<TaskView>),

    /// A task in the active agent's workspace was started or moved on
    UpsertTask(TaskView),
}

/// A plan the agent wrote, as the todo list above the input shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanView {
    /// Workspace-relative path of the plan file
    pub path: String,
    /// Plan title
    pub name: String,
    /// Steps, in order
    pub todos: Vec<TodoView>,
}

impl PlanView {
    /// `(closed, total)` todo counts.
    #[must_use]
    pub fn progress(&self) -> (usize, usize) {
        let closed = self.todos.iter().filter(|t| t.state.is_closed()).count();
        (closed, self.todos.len())
    }

    /// Whether some todo is still pending or in progress.
    #[must_use]
    pub fn has_open_todos(&self) -> bool {
        self.todos.iter().any(|t| !t.state.is_closed())
    }
}

/// Where a task stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// An agent is working on it
    #[default]
    Running,
    /// Stopped with work left: the turn ended or the app closed first
    Open,
    /// Done
    Completed,
    /// The agent failed at it
    Failed,
    /// Stopped by the user
    Cancelled,
}

impl TaskState {
    /// Done, failed or stopped: no agent will move it on.
    #[must_use]
    pub const fn is_closed(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// A piece of work an agent did or is doing: a user's request, or what
/// an agent handed to another with `task`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskView {
    /// Task id, the name of its file under `.aura/tasks`
    pub id: String,
    /// What was asked, first line
    pub title: String,
    pub state: TaskState,
    /// Hex id and name of the agent doing it
    pub agent_id: String,
    pub agent_name: String,
    /// Kind of agent it was handed to, for a spawned task
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Id of the task this one is part of
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Workspace-relative path of the plan it works on, and that plan's
    /// todo it does
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_todo: Option<String>,
    /// What the agent has done and has left to do
    #[serde(default)]
    pub items: Vec<TodoView>,
    /// What the agent said it did, once it is done
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// When it last moved on, in ms since the epoch
    pub updated_ms: u64,
}

impl TaskView {
    /// `(closed, total)` item counts.
    #[must_use]
    pub fn progress(&self) -> (usize, usize) {
        let closed = self.items.iter().filter(|t| t.state.is_closed()).count();
        (closed, self.items.len())
    }
}

/// An image pasted into a user message.
#[derive(Clone, PartialEq, Eq)]
pub struct ImageAttachment {
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`
    pub media_type: String,
    /// The image, base64-encoded
    pub data: String,
    /// Size of the image before encoding
    pub bytes: usize,
}

impl std::fmt::Debug for ImageAttachment {
    // The data is megabytes of base64; logging it would bury everything else.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageAttachment")
            .field("media_type", &self.media_type)
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

/// One entry in the model selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelOptionView {
    /// Model id sent back in [`UiEvent::SelectModel`]
    pub id: String,
    /// Display label
    pub label: String,
    /// Whether the model runs on the local server (no login required)
    pub local: bool,
    /// For local models: whether it is installed on the local server
    pub installed: bool,
    /// Selector group heading (e.g. "Cloud (aura-router)")
    pub group: String,
    /// Provider that must be signed in (`zos`, `claude`, `chatgpt`), if any
    pub provider: Option<String>,
    /// Whether the required sign-in is present
    pub available: bool,
    /// Context window in tokens; `None` for `gate`, whose window is that
    /// of each turn's pick
    #[serde(default)]
    pub context_window: Option<u64>,
}

/// Data for displaying a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageData {
    /// Message role (user or assistant)
    pub role: MessageRole,
    /// Message content
    pub content: String,
    /// Whether the message is still streaming
    pub is_streaming: bool,
    /// When the message was originally sent (ms since epoch); `None` means now
    #[serde(default)]
    pub timestamp_ms: Option<u64>,
    /// Shell command the user ran, shown as a terminal prompt line
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Agent step (thinking or a tool call), shown as a collapsible block
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<ActivityData>,
    /// The models of the turn an assistant reply ends (see
    /// [`UiCommand::SetReplyModel`])
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Tokens and cost of that turn's model calls
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<CallUsage>,
    /// Those tokens split by the sign-in that paid for them
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceUsage>,
}

/// Tokens of a turn that one sign-in paid for: `Claude plan`,
/// `OpenRouter`, `aura-router`, `ChatGPT plan` or `local`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceUsage {
    /// Who paid, as the footer names it
    pub label: String,
    /// Whole prompt (cached or not) plus output of the calls it paid for
    pub tokens: u64,
    /// Estimated cost of those calls at list prices; `None` when none was priced
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// Tokens model calls used and what they cost.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CallUsage {
    /// Whole prompt (cached or not) plus output
    pub tokens: u64,
    /// Estimated cost at API list prices; `None` for local or unpriced models
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// Tokens and cost of one model call, for the session totals.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallStats {
    /// Model the call went to
    pub model: String,
    /// Prompt tokens neither read from nor written to the cache
    pub input_tokens: u64,
    /// Prompt tokens read from the cache
    pub cache_read_tokens: u64,
    /// Prompt tokens written to the cache
    pub cache_write_tokens: u64,
    /// Output tokens
    pub output_tokens: u64,
    /// Estimated cost at API list prices; `None` for local or unpriced models
    pub cost_usd: Option<f64>,
    /// Context window of the model, when known
    pub context_limit: Option<u64>,
}

impl CallStats {
    /// The whole prompt: what the call put in the context window.
    #[must_use]
    pub const fn prompt_tokens(&self) -> u64 {
        self.input_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

/// Usage of a session so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionTotals {
    /// Model calls made
    pub calls: u32,
    /// Prompt tokens neither read from nor written to the cache
    pub input_tokens: u64,
    /// Prompt tokens read from the cache
    pub cache_read_tokens: u64,
    /// Prompt tokens written to the cache
    pub cache_write_tokens: u64,
    /// Output tokens
    pub output_tokens: u64,
    /// Estimated cost; `None` when no call had a price
    pub cost_usd: Option<f64>,
    /// Prompt of the latest call
    pub context_tokens: u64,
    /// Models the calls went to, first use first
    pub models: Vec<String>,
}

/// Status of a todo item.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoState {
    /// Not started
    #[default]
    Pending,
    /// Being worked on
    InProgress,
    /// Done
    Completed,
    /// Dropped
    Cancelled,
}

impl TodoState {
    /// Done or dropped: nothing left to do.
    #[must_use]
    pub const fn is_closed(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

/// One item of an agent's todo list, or a step of a [`PlanView`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoView {
    /// What needs doing
    pub content: String,
    /// Where it stands
    pub state: TodoState,
    /// The agent it was given to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Id of the task doing it, for a plan's todo
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// A background job an agent started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobView {
    /// Job id the agent uses, e.g. `job-1`
    pub id: String,
    /// The command line
    pub command: String,
    /// Whether it is still running
    pub running: bool,
}

/// One message or command waiting in an agent's queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedItemView {
    /// Id of the entry, stable while it waits; events naming the entry
    /// use it
    pub id: u64,
    /// The message as typed, or a label for other work (`/compact`,
    /// `! cmd`, `/build path`)
    pub text: String,
    /// Whether the entry is a message whose text the user may edit
    pub editable: bool,
}

/// Estimated make-up of an agent's context.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextView {
    /// System prompt
    pub system_tokens: u64,
    /// Tool definitions
    pub tools_tokens: u64,
    /// Skills
    pub skills_tokens: u64,
    /// The conversation so far
    pub conversation_tokens: u64,
    /// Everything together
    pub total_tokens: u64,
    /// Context window of the model, when known
    pub limit_tokens: Option<u64>,
}

/// A project instruction file (`AGENTS.md`, `CLAUDE.md`, a Cursor rule)
/// and how the agent's system prompt uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionFileView {
    /// Where it is, with the home directory shortened to `~`
    pub path: String,
    /// Bytes of it in the system prompt
    pub bytes: usize,
    /// Globs a scoped rule applies to
    pub globs: Option<String>,
    /// How it is used
    pub status: InstructionStatus,
}

/// The policy an agent's kernel enforces, as the Kernel panel's POLICIES
/// tab shows it. A read-only snapshot: it changes only when the event loop
/// sends another.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyView {
    /// Work mode, the session ceiling every tool call is checked against
    pub mode: PermissionMode,
    /// Which allowed calls ask first
    pub approval: ApprovalLevel,
    /// BLAKE3 hex digest of the policy configuration in force
    pub policy_hash: String,
    /// Proposal kinds the kernel accepts (`reason`, `delegate`, ...)
    pub action_kinds: Vec<String>,
    /// Most tool calls accepted from one model reply
    pub max_proposals: usize,
    /// Capabilities the agent holds (`SpawnAgent`, `InvokeProcess`, ...)
    pub capabilities: Vec<String>,
    /// Capabilities there are to hold, for `n/total`
    pub capability_total: usize,
    /// Orgs, projects and agents the agent may touch, in words
    pub scope: String,
    /// How each tool offered to the agent resolves now, in name order
    pub tools: Vec<ToolPolicyView>,
    /// The project's "always allow" rules, as their labels
    pub always_allow: Vec<String>,
    /// Answers remembered for this session, as `tool=state`
    pub session_grants: Vec<String>,
    /// Integrations installed for this runtime
    pub integrations: Vec<String>,
    /// Whether commands can be confined to the agent's folder here
    pub commands_confined: bool,
    /// Whether `ask` calls can prompt the user; without it they are denied
    pub live_prompts: bool,
    /// How long a prompt waits before its call is denied, in seconds
    pub prompt_timeout_secs: u64,
    /// `PermissionRequest` plugin hooks consulted before prompting
    pub permission_hooks: usize,
}

impl PolicyView {
    /// How many tools resolve to `on`, `ask` and `off`.
    #[must_use]
    pub fn tool_counts(&self) -> (usize, usize, usize) {
        self.tools
            .iter()
            .fold((0, 0, 0), |(on, ask, off), tool| match tool.state {
                ToolPolicyState::On => (on + 1, ask, off),
                ToolPolicyState::Ask => (on, ask + 1, off),
                ToolPolicyState::Off => (on, ask, off + 1),
            })
    }
}

/// How one tool resolves under an agent's policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPolicyView {
    /// Tool name
    pub name: String,
    pub state: ToolPolicyState,
    /// The deciding rule, rendered as `source.rule` (`mode.plan`,
    /// `user_default.per_tool[read_file]=on`)
    pub decided_by: String,
}

/// What the policy does with a call to a tool, before its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPolicyState {
    /// Runs without asking
    On,
    /// Asks the user first
    Ask,
    /// Denied
    Off,
}

impl ToolPolicyState {
    /// Wire label: `on`, `ask` or `off`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Ask => "ask",
            Self::Off => "off",
        }
    }
}

/// One message between two agents, for the Kernel panel's INBOX tab.
/// Messages with the same `thread_id` are one conversation, the way an
/// email subject groups mail.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailView {
    /// Unique per message, in the order messages were sent
    pub id: u64,
    /// The conversation the message belongs to
    pub thread_id: String,
    /// The conversation's subject
    pub subject: String,
    /// Hex id of the sender
    pub from_id: String,
    /// Sender's display name
    pub from_name: String,
    /// Hex id of the recipient
    pub to_id: String,
    /// Recipient's display name
    pub to_name: String,
    /// An answer or a finished task's result, rather than a new request
    pub reply: bool,
    /// Message body
    pub text: String,
    /// When it was sent, in Unix milliseconds
    pub sent_ms: u64,
}

/// How the system prompt uses an instruction file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstructionStatus {
    /// Included whole
    Included,
    /// Only its start is included; the whole file has this many bytes
    Truncated(usize),
    /// Listed for the agent to read when its description matches the work
    OnRequest,
    /// Left out because the size limit was reached
    Omitted,
}

impl MessageData {
    /// A finished message with `role` and `content`, sent now.
    #[must_use]
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            is_streaming: false,
            timestamp_ms: None,
            command: None,
            activity: None,
            model: None,
            usage: None,
            sources: Vec::new(),
        }
    }
}

/// A finished agent step, as restored from the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityData {
    /// Model thinking
    Thinking {
        /// Thinking text
        text: String,
        /// How long the model thought
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    /// A tool call
    Tool {
        /// Tool use ID
        id: String,
        /// Tool name
        name: String,
        /// Arguments as JSON
        args: String,
        /// Output, if the tool ran
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        /// Whether the call failed or was denied
        #[serde(default)]
        is_error: bool,
        /// How long the call took
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        /// Policy verdict and deciding rule, e.g. `allowed by user.on`
        #[serde(default, skip_serializing_if = "Option::is_none")]
        policy: Option<String>,
    },
}

/// Message role for display purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageRole {
    /// User message
    User,
    /// Assistant (AI) message
    Assistant,
    /// System message
    System,
}

/// Data for displaying a tool card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolData {
    /// Tool use ID
    pub id: String,
    /// Tool name
    pub name: String,
    /// Tool arguments as JSON string
    pub args: String,
}

/// Most records the Record panel holds, and the most history loaded into
/// it, newest kept. A memory guard rather than a display limit: a
/// [`RecordSummary`] can carry up to ~20KB of pretty-printed payload in
/// `message`, so this bounds the panel to a few hundred MB at worst.
pub const MAX_RECORDS: usize = 10_000;

/// Summary of a kernel record for display in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordSummary {
    /// Sequence number
    pub seq: u64,
    /// Formatted timestamp (HH:MM:SS)
    pub timestamp: String,
    /// Full context hash (hex encoded)
    pub full_hash: String,
    /// Last 4 characters of the context hash
    pub hash_suffix: String,
    /// Transaction kind (e.g., `UserPrompt`, `ActionResult`)
    pub tx_kind: String,
    /// Sender/user who initiated the transaction
    pub sender: String,
    /// Message content (truncated for display)
    pub message: String,
    /// Number of actions in this record
    pub action_count: usize,
    /// Effect status summary (e.g., "2 ok", "1 ok, 1 failed")
    pub effect_status: String,
    /// Status indicator (ok, err, pending, or -)
    pub status: RecordStatus,
    /// Additional info (e.g., tool name)
    pub info: String,
    /// Record panel column: what the record is about (tool name, LLM stop
    /// reason, sender). Empty means the panel shows `info` instead.
    #[serde(default)]
    pub what: String,
    /// Record panel column: category of `what` (tool group, LLM error
    /// class or model family).
    #[serde(default)]
    pub group: String,
    /// Record panel text after the columns, without what they already show.
    #[serde(default)]
    pub detail: String,
    /// Error details if status is Error
    #[serde(default)]
    pub error_details: String,

    // Transaction fields
    /// Transaction ID (hex encoded)
    #[serde(default)]
    pub tx_id: String,
    /// Agent ID (hex encoded)
    #[serde(default)]
    pub agent_id: String,
    /// Raw timestamp in milliseconds since epoch
    #[serde(default)]
    pub ts_ms: u64,
    /// Policy provenance, set for policy decision entries.
    #[serde(default)]
    pub policy: Option<PolicyDetail>,
}

/// Why the kernel allowed, denied, or paused a tool call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDetail {
    /// Tool the decision applies to
    pub tool: String,
    /// `approved`, `denied`, or `pending_approval`
    pub verdict: String,
    /// The deciding rule, rendered as `source.rule`
    pub decided_by: String,
    /// Denial / pending reason, empty when approved
    pub reason: String,
    /// Evaluated gates in order, each rendered as `outcome source.rule`
    pub trace: Vec<String>,
    /// Live approver (`hook` or `user`, with remember scope), empty if none
    pub approval: String,
    /// Agent mode at decision time
    pub mode: String,
    /// Hash of the policy configuration that was evaluated
    pub policy_hash: String,
    /// Hash of the `ToolProposal` transaction this decision answers
    pub proposal_tx: String,
}

/// Status of a record entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    /// All effects completed successfully
    Ok,
    /// One or more effects failed
    Error,
    /// Effects are pending
    Pending,
    /// No effects (neutral status)
    None,
}

/// Summary of a permanent agent for display in the roster.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDefinitionSummary {
    /// Name agents spawn it by
    pub name: String,
    /// What it does and when to spawn it
    pub description: String,
    /// Its unique abbreviation, the prefix of its instances' codes (`RE`
    /// for `rust-engineer`, whose instances are `RE1`, `RE2`, …), when
    /// known
    #[serde(default)]
    pub abbreviation: Option<String>,
}

/// Summary of an agent for display in the Swarm panel.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentSummary {
    /// Agent ID (hex encoded)
    pub id: String,
    /// Display name
    pub name: String,
    /// ZNS identifier (e.g., "0://Agent09")
    pub zns_id: String,
    /// Whether this agent is currently active
    pub is_active: bool,
    /// Number of records for this agent
    pub record_count: u64,
    /// Last activity timestamp (HH:MM:SS or date)
    pub last_active: String,
    /// Time of its latest record, in Unix milliseconds
    #[serde(default)]
    pub last_active_ms: Option<u64>,
    /// Permanent agent this is an instance of
    #[serde(default)]
    pub definition: Option<String>,
    /// How many spawning agents it is nested under (0 for top-level agents)
    #[serde(default)]
    pub depth: usize,
    /// Whether it is working right now: running a turn, a command, or a
    /// task for the agent that spawned it
    #[serde(default)]
    pub running: bool,
    /// Whether it is a run of a benchmark, or works under one
    #[serde(default)]
    pub bench_run: bool,
    /// Id of the project it is in. A project's agents are listed together.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Name of the project it is in
    #[serde(default)]
    pub project: Option<String>,
    /// Whether it is in plan mode: it researches and writes a plan instead
    /// of changing code
    #[serde(default)]
    pub plan_mode: bool,
    /// Short code name, such as `RE36`, unique among the agents and never
    /// changed; shown at the right end of the agent's row
    #[serde(default)]
    pub code: Option<String>,
    /// Short name of the subtask a spawned agent works on, such as
    /// `Fix parser escapes`. `name` already shows it after the code
    /// (`RE36: Fix parser escapes`); kept apart so UIs can style it.
    #[serde(default)]
    pub subtitle: Option<String>,
}

impl AgentSummary {
    /// Key of the project it is listed in: the project's id, or its own id
    /// when it has none.
    #[must_use]
    pub fn project_key(&self) -> &str {
        self.project_id.as_deref().unwrap_or(&self.id)
    }
}

/// Whether `agents[index]` is the first agent of its project, in a list
/// that lists each project's agents together. An agent without a project
/// starts one of its own unless it was spawned.
#[must_use]
pub fn starts_project(agents: &[AgentSummary], index: usize) -> bool {
    let agent = &agents[index];
    match &agent.project_id {
        Some(id) => index == 0 || agents[index - 1].project_id.as_ref() != Some(id),
        None => agent.depth == 0,
    }
}

/// Where the projects with no agent in `agents`, every one of theirs
/// being archived, go among the projects of `agents`: for each,
/// the index of the agent it goes before (`agents.len()` for the end) and
/// its index in `projects`, in list order. Each keeps its place: as many
/// projects of `agents` come before it as come before it in `projects`,
/// however those were reordered since.
#[must_use]
pub fn empty_projects(projects: &[ProjectSummary], agents: &[AgentSummary]) -> Vec<(usize, usize)> {
    let starts: Vec<usize> = (0..agents.len())
        .filter(|&i| starts_project(agents, i))
        .collect();
    let shown = |project: &ProjectSummary| agents.iter().any(|a| a.project_key() == project.id);
    let mut above = 0;
    let mut out = Vec::new();
    for (at, project) in projects.iter().enumerate() {
        if shown(project) {
            above += 1;
        } else {
            let before = starts.get(above).copied().unwrap_or(agents.len());
            out.push((before, at));
        }
    }
    out
}

/// A project of the projects list: a folder its agents work in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSummary {
    /// Project id
    pub id: String,
    /// What it is called: its folder's name
    pub name: String,
    /// Full path of its folder
    pub folder: String,
    /// Whether the folder is still there
    pub folder_exists: bool,
    /// When it was made, in local time
    pub created: String,
    /// How many of its agents are listed
    pub listed_agents: usize,
    /// How many of its agents sit in the Archive
    pub archived_agents: usize,
}

/// What a row in the Processes tab is running.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessKind {
    /// A turn of an agent the user talks to
    #[default]
    Turn,
    /// A command the user typed at the terminal prompt
    Shell,
    /// A permanent agent doing a `task` for the agent that spawned it
    Task,
    /// A background agent, started by `spawn_agent` or a detached `task`
    Agent,
    /// A command an agent started, or a process it forked
    Command,
}

impl ProcessKind {
    /// Short name shown in the list and the detail view.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Shell => "shell",
            Self::Task => "task",
            Self::Agent => "agent",
            Self::Command => "command",
        }
    }
}

/// Where a process is in its life.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    /// Working right now
    #[default]
    Running,
    /// Alive and waiting for a message (background agents)
    Idle,
    /// Holding its messages until resumed (background agents)
    Paused,
    /// Finished on its own
    Exited,
    /// Finished with an error or a non-zero exit code
    Failed,
    /// Stopped by the user or its parent
    Killed,
}

impl ProcessStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Idle => "idle",
            Self::Paused => "paused",
            Self::Exited => "exited",
            Self::Failed => "failed",
            Self::Killed => "killed",
        }
    }

    /// Whether the process has not finished yet.
    #[must_use]
    pub const fn is_live(self) -> bool {
        matches!(self, Self::Running | Self::Idle | Self::Paused)
    }
}

/// One row of the Processes tab, with the metadata its detail view shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessSummary {
    /// Unique id, sent back in [`UiEvent::KillProcess`]
    pub id: String,
    /// What it is
    pub kind: ProcessKind,
    /// One-line name: the command, or the agent's name
    pub label: String,
    /// Full command line or prompt
    #[serde(default)]
    pub detail: String,
    /// Where it is in its life
    pub status: ProcessStatus,
    /// Agent that started it (hex id)
    pub owner_agent_id: String,
    /// Display name of that agent
    pub owner_name: String,
    /// Agents above the owner, nearest first (hex ids)
    #[serde(default)]
    pub parent_chain: Vec<String>,
    /// Indentation level in the list
    #[serde(default)]
    pub depth: usize,
    /// When it started, in Unix milliseconds
    pub started_ms: u64,
    /// When it finished, in Unix milliseconds
    #[serde(default)]
    pub ended_ms: Option<u64>,
    /// Model tool call that started it
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// Hash of the transaction that started it
    #[serde(default)]
    pub spawn_tx: Option<String>,
    /// OS process id
    #[serde(default)]
    pub pid: Option<u32>,
    /// Working directory
    #[serde(default)]
    pub cwd: Option<String>,
    /// Model an agent runs on
    #[serde(default)]
    pub model: Option<String>,
    /// Turns an agent has finished
    #[serde(default)]
    pub turns: Option<u32>,
    /// Messages waiting for an agent
    #[serde(default)]
    pub queued: Option<usize>,
    /// Exit code of a finished command
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// File a background command writes its output to
    #[serde(default)]
    pub log_path: Option<PathBuf>,
    /// Agent in the Projects list to open for this process: the owner, or
    /// the terminal agent at the root of a background agent's tree
    #[serde(default)]
    pub jump_agent_id: String,
    /// Whether the backend can kill it
    #[serde(default)]
    pub killable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_policy_view_counts_its_tools_by_state() {
        let tool = |name: &str, state| ToolPolicyView {
            name: name.into(),
            state,
            decided_by: String::new(),
        };
        let view = PolicyView {
            tools: vec![
                tool("read_file", ToolPolicyState::On),
                tool("list_files", ToolPolicyState::On),
                tool("write_file", ToolPolicyState::Ask),
                tool("git_push", ToolPolicyState::Off),
            ],
            ..PolicyView::default()
        };
        assert_eq!(view.tool_counts(), (2, 1, 1));
        assert_eq!(ToolPolicyState::Ask.label(), "ask");
    }

    #[test]
    fn test_ui_event_variants() {
        let event = UiEvent::UserMessage {
            agent_id: "aa".to_string(),
            text: "hello".to_string(),
            shown: true,
            images: Vec::new(),
        };
        assert!(matches!(event, UiEvent::UserMessage { .. }));

        let event = UiEvent::Approve("tool-1".to_string());
        assert!(matches!(event, UiEvent::Approve(_)));

        let event = UiEvent::Quit;
        assert!(matches!(event, UiEvent::Quit));
    }

    #[test]
    fn test_ui_command_variants() {
        let cmd = UiCommand::SetStatus("Ready".to_string());
        assert!(matches!(cmd, UiCommand::SetStatus(_)));

        let cmd = UiCommand::CompleteTool {
            id: "1".to_string(),
            result: "ok".to_string(),
            success: true,
        };
        assert!(matches!(cmd, UiCommand::CompleteTool { .. }));
    }

    #[test]
    fn test_message_role_equality() {
        assert_eq!(MessageRole::User, MessageRole::User);
        assert_ne!(MessageRole::User, MessageRole::Assistant);
    }
}

/// Health of a run-graph node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TraceNodeStatus {
    /// Still running or waiting for a result
    #[default]
    Pending,
    /// Finished normally
    Ok,
    /// Needed a nudge, retry or block
    Warn,
    /// Failed
    Error,
}

/// One node of the run graph: a run, turn, iteration, tool call,
/// harness decision, reward, subagent or branch.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TraceNode {
    /// Stable id within the run
    pub id: String,
    /// Kind label (`run`, `turn`, `iteration`, `tool`, `decision`, ...)
    pub kind: String,
    /// Containing node
    pub parent: Option<String>,
    /// Short label
    pub label: String,
    /// One-line detail shown after the label
    pub detail: String,
    /// Health
    pub status: TraceNodeStatus,
    /// Trajectory step the node came from
    pub seq: Option<u64>,
    /// Metadata shown in the side pane, in display order
    pub meta: Vec<(String, String)>,
    /// Non-containment edges from this node: (edge kind, other node id)
    pub links: Vec<(String, String)>,
}
