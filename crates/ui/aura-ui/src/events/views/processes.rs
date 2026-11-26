//! Rows of the Processes tab.

#[cfg(doc)]
use crate::events::UiEvent;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What a row in the Processes tab is running.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessKind {
    /// A turn of an agent the user talks to
    #[default]
    Turn,
    /// A command the user typed at the terminal prompt
    Shell,
    /// A permanent agent doing a `task` for the agent that spawned it
    Task,
    /// A background agent, started by `spawn_agent` or a detached `task`
    Agent,
    /// A command an agent started, or a process it forked
    Command,
}

impl ProcessKind {
    /// Short name shown in the list and the detail view.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Shell => "shell",
            Self::Task => "task",
            Self::Agent => "agent",
            Self::Command => "command",
        }
    }
}

/// Where a process is in its life.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    /// Working right now
    #[default]
    Running,
    /// Alive and waiting for a message (background agents)
    Idle,
    /// Holding its messages until resumed (background agents)
    Paused,
    /// Finished on its own
    Exited,
    /// Finished with an error or a non-zero exit code
    Failed,
    /// Stopped by the user or its parent
    Killed,
}

impl ProcessStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Idle => "idle",
            Self::Paused => "paused",
            Self::Exited => "exited",
            Self::Failed => "failed",
            Self::Killed => "killed",
        }
    }

    /// Whether the process has not finished yet.
    #[must_use]
    pub const fn is_live(self) -> bool {
        matches!(self, Self::Running | Self::Idle | Self::Paused)
    }
}

/// One row of the Processes tab, with the metadata its detail view shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessSummary {
    /// Unique id, sent back in [`UiEvent::KillProcess`]
    pub id: String,
    /// What it is
    pub kind: ProcessKind,
    /// One-line name: the command, or the agent's name
    pub label: String,
    /// Full command line or prompt
    #[serde(default)]
    pub detail: String,
    /// Where it is in its life
    pub status: ProcessStatus,
    /// Agent that started it (hex id)
    pub owner_agent_id: String,
    /// Display name of that agent
    pub owner_name: String,
    /// Agents above the owner, nearest first (hex ids)
    #[serde(default)]
    pub parent_chain: Vec<String>,
    /// Indentation level in the list
    #[serde(default)]
    pub depth: usize,
    /// When it started, in Unix milliseconds
    pub started_ms: u64,
    /// When it finished, in Unix milliseconds
    #[serde(default)]
    pub ended_ms: Option<u64>,
    /// Model tool call that started it
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// Hash of the transaction that started it
    #[serde(default)]
    pub spawn_tx: Option<String>,
    /// OS process id
    #[serde(default)]
    pub pid: Option<u32>,
    /// Working directory
    #[serde(default)]
    pub cwd: Option<String>,
    /// Model an agent runs on
    #[serde(default)]
    pub model: Option<String>,
    /// Turns an agent has finished
    #[serde(default)]
    pub turns: Option<u32>,
    /// Messages waiting for an agent
    #[serde(default)]
    pub queued: Option<usize>,
    /// Exit code of a finished command
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// File a background command writes its output to
    #[serde(default)]
    pub log_path: Option<PathBuf>,
    /// Agent in the Projects list to open for this process: the owner, or
    /// the terminal agent at the root of a background agent's tree
    #[serde(default)]
    pub jump_agent_id: String,
    /// Whether the backend can kill it
    #[serde(default)]
    pub killable: bool,
}
