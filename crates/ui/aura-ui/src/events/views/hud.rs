//! What the chat shows about an agent's work: todos, jobs, queue, context
//! and instruction files.

#[cfg(doc)]
use crate::events::PlanView;
use serde::{Deserialize, Serialize};

/// Status of a todo item.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoState {
    /// Not started
    #[default]
    Pending,
    /// Being worked on
    InProgress,
    /// Done
    Completed,
    /// Dropped
    Cancelled,
}

impl TodoState {
    /// Done or dropped: nothing left to do.
    #[must_use]
    pub const fn is_closed(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

/// One item of an agent's todo list, or a step of a [`PlanView`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoView {
    /// What needs doing
    pub content: String,
    /// Where it stands
    pub state: TodoState,
    /// The agent it was given to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Id of the task doing it, for a plan's todo
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
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

/// One message or command waiting in an agent's queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedItemView {
    /// Id of the entry, stable while it waits; events naming the entry
    /// use it
    pub id: u64,
    /// The message as typed, or a label for other work (`/compact`,
    /// `! cmd`, `/build path`)
    pub text: String,
    /// Whether the entry is a message whose text the user may edit
    pub editable: bool,
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
