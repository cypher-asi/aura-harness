use super::BlobId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What the model saw for one call. Every field except `params` is a
/// blob reference so the exact request can be rebuilt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelObservation {
    /// System prompt.
    pub system: BlobId,
    /// Tool definitions, serialized as a JSON array.
    pub tools: BlobId,
    /// One blob per message, in order.
    pub messages: Vec<BlobId>,
    /// Sampling parameters (model, max tokens, temperature, thinking).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

/// Coarse purpose of a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolIntent {
    /// Reading or searching the workspace.
    Explore,
    /// Mutating files.
    Edit,
    /// Building, testing or linting.
    Verify,
    /// Planning or task bookkeeping.
    Plan,
    /// Starting or messaging other agents.
    Delegate,
    /// Declaring the task finished.
    Complete,
    /// Anything else (arbitrary commands, network tools).
    Other,
}

impl ToolIntent {
    /// Classify a tool call by name and input.
    #[must_use]
    pub fn classify(name: &str, input: &Value) -> Self {
        match name {
            "read_file" | "list_files" | "search_code" | "find_files" | "stat_file" | "glob"
            | "grep" | "web_search" | "web_fetch" | "get_task_context" => Self::Explore,
            "write_file" | "edit_file" | "delete_file" | "apply_patch" | "multi_edit" => Self::Edit,
            "submit_plan" | "todo_write" | "update_plan" => Self::Plan,
            "task" | "spawn_agent" | "delegate_task" | "send_to_agent" | "agent_lifecycle" => {
                Self::Delegate
            }
            "task_done" => Self::Complete,
            "run_command" => {
                let command = input
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if is_verify_command(&command) {
                    Self::Verify
                } else {
                    Self::Other
                }
            }
            _ => Self::Other,
        }
    }

    /// Stable snake-case name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Explore => "explore",
            Self::Edit => "edit",
            Self::Verify => "verify",
            Self::Plan => "plan",
            Self::Delegate => "delegate",
            Self::Complete => "complete",
            Self::Other => "other",
        }
    }
}

fn is_verify_command(command: &str) -> bool {
    const MARKERS: [&str; 14] = [
        "test", "build", "check", "clippy", "lint", "pytest", "tsc", "make", "fmt", "vitest",
        "jest", "mypy", "eslint", "ruff",
    ];
    MARKERS.iter().any(|m| command.contains(m))
}

/// One tool call the model asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallAction {
    /// Provider tool-use id.
    pub tool_use_id: String,
    /// Tool name.
    pub name: String,
    /// Parsed input.
    pub input: Value,
    /// Classified purpose.
    pub intent: ToolIntent,
}

/// What the model produced for one call.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModelAction {
    /// Full assistant message, when blobs are recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<BlobId>,
    /// Thinking text, when blobs are recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<BlobId>,
    /// Length of the thinking text in characters.
    #[serde(default)]
    pub thinking_chars: usize,
    /// Thinking text, kept at every capture level. Empty for runs
    /// recorded before it was kept, and for models that show none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thinking_text: String,
    /// Visible assistant text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Requested tool calls, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallAction>,
    /// Provider stop reason.
    #[serde(default)]
    pub stop_reason: String,
    /// What the harness made the call for (`classify`, `criteria`,
    /// `judge`); `None` for the agent's own calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Why the call failed, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Token accounting for one model call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Uncached prompt tokens.
    pub input: u64,
    /// Generated tokens.
    pub output: u64,
    /// Prompt tokens written to the cache.
    #[serde(default)]
    pub cache_write: u64,
    /// Prompt tokens read from the cache.
    #[serde(default)]
    pub cache_read: u64,
}

/// One complete model call, as emitted by the agent loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelExchange {
    /// Zero-based iteration.
    pub iteration: u32,
    /// Model that served the call.
    pub model: String,
    /// What the model saw. `None` when blob capture is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<ModelObservation>,
    /// What the model produced.
    pub action: ModelAction,
    /// Token accounting.
    pub usage: TokenUsage,
    /// Wall time of the call.
    pub latency_ms: u64,
    /// Provider correlation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
}
