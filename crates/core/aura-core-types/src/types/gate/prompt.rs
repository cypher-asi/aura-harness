//! The payload of a `UserPrompt` transaction.

use super::category::{stored_category, Difficulty, PromptCategory};
use super::route::Route;
use super::to_json;
use serde::{Deserialize, Serialize};

/// Payload of a `UserPrompt` transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptPayload {
    /// The prompt as typed (or as the harness wrote it).
    pub text: String,
    /// What the prompt asks for; `None` when Gate does not route its
    /// work (a pinned model, a message sent into a running turn).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "stored_category"
    )]
    pub category: Option<PromptCategory>,
    /// Model that classified the prompt; empty when nothing did.
    pub classifier: String,
    /// Where the prompt's work runs.
    pub route: Route,
    /// The kind of agent a subtask went to (the permanent agent the
    /// `task` call named); `None` for a turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Whether the turn was led: its model planned the work and handed it
    /// to subtasks instead of doing it. `None` for a subtask, and for a
    /// turn recorded before this was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led: Option<bool>,
    /// How hard the classifier judged the work; `None` when it did not
    /// say, and for prompts recorded before it was asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<Difficulty>,
}

impl PromptPayload {
    /// `text`, not classified, with no route recorded.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            category: None,
            classifier: String::new(),
            route: Route::default(),
            kind: None,
            led: None,
            difficulty: None,
        }
    }

    /// The payload as stored on the transaction.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        to_json(self)
    }

    /// Read a stored payload.
    ///
    /// # Errors
    /// Returns the JSON error when `bytes` is not a prompt payload.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}
