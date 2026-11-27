//! Commands the application logic sends to the UI.

use super::{
    AgentDefinitionSummary, AgentSummary, BenchExample, BenchSession, BenchView, BootUpdate,
    CallStats, CallUsage, ChannelPostView, ChannelSummary, ContextView, FileDiffData, FlowLane,
    FlowNode, InstructionFileView, JobView, LogView, MailView, MessageData, ModelOptionView,
    PlanView, PolicyView, ProcessSummary, ProjectSummary, QueuedItemView, RecordSummary,
    SessionTotals, SourceUsage, TaskView, TodoView, ToolData, TraceNode,
};
#[cfg(doc)]
use super::{UiEvent, MAX_RECORDS};
use crate::mode_command::{ApprovalLevel, PermissionMode};
use std::path::PathBuf;

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

    /// What a running command call just printed, shown in the call's
    /// block until its result arrives
    ToolOutput {
        /// Tool use ID
        id: String,
        /// The next piece of output, possibly with ANSI styling
        chunk: String,
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

    /// A machine coming up, or an agent moving between machines, shown as
    /// a live boot console in the chat. The first update for an `id` adds
    /// the console; later ones change it.
    Boot {
        /// Console id
        id: String,
        /// What changed
        update: BootUpdate,
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

    /// What the shell command from the last [`UiEvent::RunCommand`] just
    /// printed, while it is still running
    CommandOutput {
        /// The next piece of terminal output, possibly with ANSI styling
        chunk: String,
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
    /// A better subject for thread `thread_id`, written by the title model
    /// after its first message. Global like [`Self::AddMail`]; it replaces
    /// the subject the thread's messages carry, whichever arrives first.
    SetThreadSubject {
        /// The thread named
        thread_id: String,
        /// Its subject, in place of the one its messages carry
        subject: String,
    },

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

    /// Show the active agent's machine in the Sandbox panel, its
    /// workspace there at this path: directories are listed by asking
    /// ([`UiEvent::ListSandbox`]), never read here
    SetSandboxRemote(PathBuf),

    /// What a directory of the active agent's machine holds, asked for
    /// with [`UiEvent::ListSandbox`]
    SandboxListing {
        /// The directory, as asked for
        dir: PathBuf,
        /// Its entries, or why it could not be listed
        entries: Result<Vec<crate::sandbox::RemoteEntry>, String>,
    },

    /// A file of the active agent's machine, asked for with
    /// [`UiEvent::ReadSandboxFile`]
    SandboxFile {
        /// The file, as asked for
        path: PathBuf,
        /// Its text, or why it could not be read
        text: Result<String, String>,
    },

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

    /// Replace the BENCH tab's list with every benchmark, of every tree,
    /// oldest first
    BenchList { benches: Vec<BenchView> },

    /// A benchmark started or changed; add it to the BENCH tab or replace
    /// it there
    BenchUpsert(Box<BenchView>),

    /// The sessions of `agent_id` the BENCH tab can replay, newest first,
    /// and the built-in example tasks it can run, in the order a
    /// benchmark takes them. Ignored by a form starting from another
    /// agent.
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

    /// Where an agent runs, for its footer switch
    SetPlacement {
        /// Agent (hex id)
        agent_id: String,
        state: crate::host::PlacementState,
    },

    /// The HOST tab of the agent on screen
    SetHost(Box<crate::host::HostView>),

    /// The REPO tab of an agent; dropped unless it is the one on screen
    SetRepo(Box<crate::repo::RepoView>),

    /// Every channel that is not archived, in list order
    SetChannels(Vec<ChannelSummary>),

    /// Every task of a channel's agents, newest first
    SetChannelTasks {
        /// Channel id
        channel_id: String,
        tasks: Vec<TaskView>,
    },

    /// A new post in a channel
    ChannelPost {
        /// Channel id
        channel_id: String,
        post: ChannelPostView,
    },

    /// Every post of a channel, oldest first
    SetChannelPosts {
        /// Channel id
        channel_id: String,
        posts: Vec<ChannelPostView>,
    },

    /// The merge queue of a channel's repo, for its QUEUE tab
    SetMergeQueue {
        /// Channel id
        channel_id: String,
        queue: crate::events::MergeQueueView,
    },
}
