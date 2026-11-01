//! UI event types for communication between UI and kernel.
//!
//! The event system uses channels for bidirectional communication:
//! - `UiEvent`: Events from the UI to the kernel (user actions)
//! - `UiCommand`: Commands from the kernel to the UI (state updates)

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Events sent from the UI to the application logic.
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// User submitted a message
    UserMessage(String),

    /// User typed a shell command to run in the agent's workspace
    RunCommand {
        /// Command line
        command: String,
        /// Width of the terminal the command's output is shown in
        cols: u16,
    },

    /// User approved a pending tool request
    Approve(String),

    /// User denied a pending tool request
    Deny(String),

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

    /// User cancelled the current operation
    Cancel,

    /// User requested a new session (reset context)
    NewSession,

    /// User selected a different agent
    SelectAgent(String),

    /// User requested the agent list
    RefreshAgents,

    /// User requested a new agent (with optional display name)
    CreateAgent(Option<String>),

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

    /// Request user approval for a tool
    RequestApproval {
        /// Tool use ID
        id: String,
        /// Tool name
        tool: String,
        /// Description of the action
        description: String,
    },

    /// Show an error notification
    ShowError(String),

    /// Show a success notification
    ShowSuccess(String),

    /// Show a warning notification
    ShowWarning(String),

    /// Mark the current operation as complete
    Complete,

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
    /// Permanent agent this is an instance of
    #[serde(default)]
    pub definition: Option<String>,
    /// How many spawning agents it is nested under (0 for top-level agents)
    #[serde(default)]
    pub depth: usize,
    /// Whether it is running a task for the agent that spawned it
    #[serde(default)]
    pub running: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_event_variants() {
        let event = UiEvent::UserMessage("hello".to_string());
        assert!(matches!(event, UiEvent::UserMessage(_)));

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
