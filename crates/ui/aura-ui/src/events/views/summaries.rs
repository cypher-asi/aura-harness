//! One-line summaries of the user's long channel messages, for the
//! channel's HISTORY tab.

use serde::{Deserialize, Serialize};

/// A message the UI wants summarized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSummaryRequest {
    /// What the UI files the answer under; sent back unchanged
    pub key: String,
    /// The message, as the user sent it
    pub text: String,
}

/// The answer to a [`MessageSummaryRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSummaryView {
    /// The request's key
    pub key: String,
    /// The one-line summary; `None` when the model gave none, failed or
    /// took too long, so the UI keeps its own cut of the message
    pub summary: Option<String>,
}
