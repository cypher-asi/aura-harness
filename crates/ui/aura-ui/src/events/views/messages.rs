//! Chat messages, tool cards, file diffs, pasted images and the model selector.

use crate::events::{BootHistory, CallUsage, SourceUsage};
#[cfg(doc)]
use crate::events::{UiCommand, UiEvent};
use serde::{Deserialize, Serialize};

/// How one file changed, as a unified diff of its hunks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiffData {
    /// File, relative to the agent's workspace when inside it
    pub path: String,
    /// Unified diff hunks (`@@` headers and `+`/`-`/` ` lines), or a note
    /// such as for a binary file
    pub diff: String,
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
    /// A boot console, shown again as it was
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot: Option<BootHistory>,
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
            boot: None,
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
