use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::schedule::Schedule;

#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct AutomatonId(String);

impl AutomatonId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for AutomatonId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AutomatonId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomatonStatus {
    Installing,
    Running,
    Paused,
    Stopped,
    Failed,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomatonInfo {
    pub id: AutomatonId,
    pub kind: String,
    pub status: AutomatonStatus,
    pub schedule: Schedule,
    pub config: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// TODO: TaskExecution and related types are reserved for task history tracking
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecution {
    pub task_id: String,
    pub agent_instance_id: String,
    pub session_id: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub outcome: Option<TaskOutcome>,
    pub file_ops: Vec<FileOpRecord>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskOutcome {
    Success { summary: String },
    Failed { reason: String },
    Skipped { reason: String },
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOpRecord {
    pub path: String,
    pub op_type: FileOpType,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOpType {
    Created,
    Modified,
    Deleted,
}

// `FollowUpSuggestion` is defined canonically in
// [`aura_agent::agent_runner::FollowUpSuggestion`]. Re-exported here so
// existing callers referencing `aura_automaton::types::FollowUpSuggestion`
// continue to compile without duplicating the struct definition.
#[allow(unused_imports)]
pub use aura_agent::agent_runner::FollowUpSuggestion;
