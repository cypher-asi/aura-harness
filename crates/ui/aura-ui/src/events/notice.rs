//! The system lines commands add to a chat: errors, successes, warnings
//! and plain system messages.

use super::{MessageRole, UiCommand};

/// What the chat line of a [`UiCommand::ShowError`] starts with.
pub const ERROR_PREFIX: &str = "error: ";
/// What the chat line of a [`UiCommand::ShowSuccess`] starts with.
pub const SUCCESS_PREFIX: &str = "✓ ";
/// What the chat line of a [`UiCommand::ShowWarning`] starts with.
pub const WARNING_PREFIX: &str = "warning: ";

impl UiCommand {
    /// The system line this command adds to a chat, when it is a notice:
    /// an error, success or warning, or a plain system message.
    #[must_use]
    pub fn chat_notice(&self) -> Option<String> {
        match self {
            Self::ShowError(msg) => Some(format!("{ERROR_PREFIX}{msg}")),
            Self::ShowSuccess(msg) => Some(format!("{SUCCESS_PREFIX}{msg}")),
            Self::ShowWarning(msg) => Some(format!("{WARNING_PREFIX}{msg}")),
            Self::ShowMessage(data)
                if data.role == MessageRole::System
                    && data.activity.is_none()
                    && data.command.is_none()
                    && data.boot.is_none() =>
            {
                Some(data.content.clone())
            }
            _ => None,
        }
    }
}
