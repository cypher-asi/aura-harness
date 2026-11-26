//! Plans the agent wrote and the tasks agents work on.

use crate::events::TodoView;
use serde::{Deserialize, Serialize};

/// A plan the agent wrote, as the todo list above the input shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanView {
    /// Workspace-relative path of the plan file
    pub path: String,
    /// Plan title
    pub name: String,
    /// Steps, in order
    pub todos: Vec<TodoView>,
}

impl PlanView {
    /// `(closed, total)` todo counts.
    #[must_use]
    pub fn progress(&self) -> (usize, usize) {
        let closed = self.todos.iter().filter(|t| t.state.is_closed()).count();
        (closed, self.todos.len())
    }

    /// Whether some todo is still pending or in progress.
    #[must_use]
    pub fn has_open_todos(&self) -> bool {
        self.todos.iter().any(|t| !t.state.is_closed())
    }
}

/// Where a task stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// An agent is working on it
    #[default]
    Running,
    /// Stopped with work left: the turn ended or the app closed first
    Open,
    /// Done
    Completed,
    /// The agent failed at it
    Failed,
    /// Stopped by the user
    Cancelled,
}

impl TaskState {
    /// Done, failed or stopped: no agent will move it on.
    #[must_use]
    pub const fn is_closed(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// A piece of work an agent did or is doing: a user's request, or what
/// an agent handed to another with `task`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskView {
    /// Task id, the name of its file under `.aura/tasks`
    pub id: String,
    /// What was asked, first line
    pub title: String,
    pub state: TaskState,
    /// Hex id and name of the agent doing it
    pub agent_id: String,
    pub agent_name: String,
    /// Kind of agent it was handed to, for a spawned task
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Id of the task this one is part of
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Workspace-relative path of the plan it works on, and that plan's
    /// todo it does
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_todo: Option<String>,
    /// The request that builds its plan (`/build` or "implement"); the
    /// plan's header in the TASKS tab stands for it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub builds: bool,
    /// What the agent has done and has left to do
    #[serde(default)]
    pub items: Vec<TodoView>,
    /// What the agent said it did, once it is done
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// When it last moved on, in ms since the epoch
    pub updated_ms: u64,
}

impl TaskView {
    /// `(closed, total)` item counts.
    #[must_use]
    pub fn progress(&self) -> (usize, usize) {
        let closed = self.items.iter().filter(|t| t.state.is_closed()).count();
        (closed, self.items.len())
    }
}
