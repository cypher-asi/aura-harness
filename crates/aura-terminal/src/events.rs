//! UI event types for communication between UI and kernel.
//!
//! The event system uses channels for bidirectional communication:
//! - `UiEvent`: Events from the UI to the kernel (user actions)
//! - `UiCommand`: Commands from the kernel to the UI (state updates)

use serde::{Deserialize, Serialize};

/// Events sent from the UI to the application logic.
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// User submitted a message
    UserMessage(String),

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

    /// User submitted login credentials from the TUI
    LoginCredentials {
        /// Email address
        email: String,
        /// Password
        password: String,
    },

    /// User requested logout
    Logout,

    /// User requested auth status
    Whoami,
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

    /// Update a tool's completion status
    CompleteTool {
        /// Tool use ID
        id: String,
        /// Result content
        result: String,
        /// Whether the tool succeeded
        success: bool,
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

    /// Clear records (when switching agents)
    ClearRecords,

    /// Set the API status (URL and whether it's active)
    SetApiStatus {
        /// API URL
        url: Option<String>,
        /// Whether the API is active
        active: bool,
    },
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

/// Summary of an agent for display in the Swarm panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
