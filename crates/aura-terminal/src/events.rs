//! UI event types for communication between UI and kernel.
//!
//! The event system uses channels for bidirectional communication:
//! - `UiEvent`: Events from the UI to the kernel (user actions)
//! - `UiCommand`: Commands from the kernel to the UI (state updates)

use crate::mode_command::PermissionMode;
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

    /// User picked the permission mode of an agent (`/mode <name>`)
    SetMode {
        /// Agent the chat showed (hex id)
        agent_id: String,
        /// Mode for the agent's next tool calls
        mode: PermissionMode,
    },

    /// User pressed Shift+Tab to move the agent with this hex id to the
    /// next permission mode
    CycleMode(String),

    /// User requested to quit
    Quit,

    /// User requested status display
    ShowStatus,

    /// User requested help
    ShowHelp,

    /// User requested history (with optional count)
    ShowHistory(Option<usize>),

    /// User requested to clear the screen
    Clear,

    /// User cancelled the turn of the agent with this hex id
    Cancel(String),

    /// User requested a new session (reset context)
    NewSession,

    /// User selected a different agent
    SelectAgent(String),

    /// User requested the agent list
    RefreshAgents,

    /// User requested a new agent (with optional display name)
    CreateAgent(Option<String>),

    /// User picked a folder for a new project and named it. A default
    /// agent is created under it, working in that folder.
    CreateProject {
        /// Project name
        name: String,
        /// Folder the project's agents work in
        folder: PathBuf,
    },

    /// User renamed the project that contains the active agent.
    RenameProject(String),

    /// User started an instance of the named permanent agent
    InstantiateDefinition(String),

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

    /// The model that wrote the replies shown since the last user message
    /// that don't name a model yet (sent once each model call finishes)
    SetReplyModel {
        /// Model that answered the call
        model: String,
        /// Tokens and cost of the call, when the provider reported usage
        usage: Option<CallUsage>,
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

    /// The agent's permission mode
    SetMode(PermissionMode),

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
    /// How many messages and commands wait for the agent's current work
    SetQueued(usize),
    /// What the agent's context is made of, as of its last turn
    SetContext(ContextView),
    /// Usage of the agent's session so far, when it is opened or resumed
    SetSessionTotals(SessionTotals),
    /// The project instruction files the agent's turns load
    SetInstructions(Vec<InstructionFileView>),

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

    /// Update the list of agents in the swarm
    SetAgents(Vec<AgentSummary>),

    /// Set the currently active agent
    SetActiveAgent(String),

    /// Update the list of permanent agents
    SetAgentDefinitions(Vec<AgentDefinitionSummary>),

    /// Suspend the UI and open a file in the user's editor
    OpenEditor(PathBuf),

    /// Clear records (when switching agents)
    ClearRecords,

    /// Replace the conversation with an agent's history (when switching agents)
    LoadConversation(Vec<MessageData>),

    /// Set the sandbox directory shown in the Sandbox panel (the active agent's workspace)
    SetSandboxRoot(PathBuf),

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
        /// Id of the model used for the next turn
        current: String,
        /// Display name of the logged-in user, `None` when browsing as guest
        user: Option<String>,
        /// Labels of the linked plans that can serve requests (e.g. "Claude")
        plans: Vec<String>,
    },

    /// Ask the user to paste the code shown by `provider`'s sign-in page
    PromptLinkCode {
        /// Provider label shown in the prompt
        provider: String,
    },

    /// A plan link attempt finished (successfully or not); drop any model
    /// pick that was waiting on it
    EndLink,
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
    /// Model that wrote an assistant reply
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Tokens and cost of the model call that wrote an assistant reply
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<CallUsage>,
}

/// Tokens a model call used and what they cost.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CallUsage {
    /// Whole prompt (cached or not) plus output
    pub tokens: u64,
    /// Estimated cost at API list prices; `None` for local or unpriced models
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// Tokens and cost of one model call, for the session totals.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CallStats {
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
#[derive(Debug, Clone, Copy, Default, PartialEq)]
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
}

/// Status of a todo item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoState {
    /// Not started
    Pending,
    /// Being worked on
    InProgress,
    /// Done
    Completed,
    /// Dropped
    Cancelled,
}

/// One item of an agent's todo list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoView {
    /// What needs doing
    pub content: String,
    /// Where it stands
    pub state: TodoState,
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
    /// Project this root agent heads. Empty when the project is unnamed
    /// and the header should use [`Self::name`].
    #[serde(default)]
    pub project: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
