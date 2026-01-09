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
}

/// Commands sent from the application logic to the UI.
#[derive(Debug, Clone)]
pub enum UiCommand {
    /// Set the status message
    SetStatus(String),

    /// Append text to the current streaming message
    AppendText(String),

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
    /// Transaction kind (e.g., "UserPrompt", "ActionResult")
    pub tx_kind: String,
    /// Sender/user who initiated the transaction
    pub sender: String,
    /// Message content (truncated for display)
    pub message: String,
    /// Number of actions in this record
    pub action_count: usize,
    /// Effect status summary (e.g., "2 ok", "1 ok, 1 failed")
    pub effect_status: String,
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
