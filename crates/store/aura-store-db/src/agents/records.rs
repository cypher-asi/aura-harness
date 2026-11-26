//! The registry rows and the diff that writes them.

#[cfg(doc)]
use super::{derive_agent_id, AgentStore};
use aura_core_types::AgentId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// One agent of the registry, the value of an [`crate::cf::AGENTS`] row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRecord {
    /// [`derive_agent_id`] of `zns_id`; the row key.
    pub agent_id: AgentId,
    /// ZNS id the agent's identity is derived from. Never changes.
    pub zns_id: String,
    /// The agent's name, its identity in records.
    pub name: String,
    /// When the agent was registered.
    pub created_at: DateTime<Utc>,
    /// Where the agent is listed: lower comes first.
    pub position: u64,
    /// Permanent agent this is an instance of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    /// The agent that spawned this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<AgentId>,
    /// Id of the project the agent is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Folder the agent works in instead of its project's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
    /// Moved to the Archive.
    #[serde(default)]
    pub archived: bool,
    /// Taken off the list with its project but kept.
    #[serde(default)]
    pub removed: bool,
    /// Short title of what the agent's chat is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Short code name shown beside the agent, such as `RE36`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Short name of the subtask a spawned instance was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// When the agent was moved to the Archive, if it is archived and that
    /// was recorded; the Archive lists the latest first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<DateTime<Utc>>,
}

/// One project of the registry, the value of a [`crate::cf::PROJECTS`]
/// row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRecord {
    /// Project id; the row key.
    pub id: String,
    /// Canonical folder of the project.
    pub folder: PathBuf,
    /// When the project was created.
    pub created_at: DateTime<Utc>,
    /// Where the project is listed: lower comes first.
    pub position: u64,
    /// Taken off the list but kept.
    #[serde(default)]
    pub removed: bool,
}

/// Registry-wide settings, the [`crate::cf::REGISTRY_META`] rows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryMeta {
    /// Registry format version.
    #[serde(default)]
    pub version: u32,
    /// The agent that was active when the UI last switched.
    #[serde(default)]
    pub active: Option<AgentId>,
    /// The code prefix each source key (definition name, or `agent`) has
    /// reserved, e.g. `rust-engineer` -> `RE`.
    #[serde(default)]
    pub prefixes: BTreeMap<String, String>,
    /// The highest code number each prefix has ever been given.
    #[serde(default)]
    pub last_numbers: BTreeMap<String, u64>,
}

/// Everything [`AgentStore::load`] read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    /// Agents, sorted by `position`, ties by key.
    pub agents: Vec<AgentRecord>,
    /// Projects, sorted by `position`, ties by key.
    pub projects: Vec<ProjectRecord>,
    /// The settings; `None` when no meta row was ever written.
    pub meta: Option<RegistryMeta>,
}

/// Rows to write or delete in one batch.
///
/// Puts are staged before deletes, so a key in both ends up deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryDiff {
    /// Agent rows to write.
    pub agents_put: Vec<AgentRecord>,
    /// Agent rows to delete.
    pub agents_del: Vec<AgentId>,
    /// Project rows to write.
    pub projects_put: Vec<ProjectRecord>,
    /// Project rows to delete, by id.
    pub projects_del: Vec<String>,
    /// Settings to write in full; `None` leaves them as they are.
    pub meta: Option<RegistryMeta>,
}

impl RegistryDiff {
    /// Whether applying the diff would change nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.agents_put.is_empty()
            && self.agents_del.is_empty()
            && self.projects_put.is_empty()
            && self.projects_del.is_empty()
            && self.meta.is_none()
    }
}
