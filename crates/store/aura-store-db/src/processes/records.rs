//! The persisted process and run records, the create / update inputs and
//! the trigger metadata allowed off-VM.

#[cfg(doc)]
use super::ProcessStore;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A persisted process definition.
///
/// Deliberately **no derived `Debug`** — the manual impl below redacts
/// the prompt and config so the in-TEE payload cannot leak through
/// log/tracing formatting.
#[derive(Clone, Serialize, Deserialize)]
pub struct ProcessRecord {
    /// Unique process id (UUID v4 string; the store key).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Cron expression (UTC). Standard 5-field expressions are
    /// accepted and normalized; 6/7-field (with seconds / year) work
    /// as-is.
    pub cron: String,
    /// The instruction the agent executes on each run. In-TEE data —
    /// never exported off-VM.
    pub prompt: String,
    /// Optional structured config the run may consult. In-TEE data —
    /// never exported off-VM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
    /// Whether the scheduler should fire this process.
    pub enabled: bool,
    /// Creation timestamp (preserved across updates).
    pub created_at: DateTime<Utc>,
    /// Last-update timestamp.
    pub updated_at: DateTime<Utc>,
    /// When the process last ran (trigger time), if ever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<DateTime<Utc>>,
    /// Next scheduled fire time computed from [`Self::cron`] (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for ProcessRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessRecord")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("cron", &self.cron)
            .field("prompt", &"<redacted>")
            .field("config", &self.config.as_ref().map(|_| "<redacted>"))
            .field("enabled", &self.enabled)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .field("last_run_at", &self.last_run_at)
            .field("next_run_at", &self.next_run_at)
            .finish()
    }
}

/// Terminal / in-flight status of one process run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessRunStatus {
    /// The run is currently executing.
    Running,
    /// The run completed successfully.
    Success,
    /// The run failed.
    Failure,
}

/// One execution of a process. Sealed at rest like the definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRunRecord {
    /// Unique run id (UUID v4 string).
    pub run_id: String,
    /// Owning process id.
    pub process_id: String,
    /// Internal monotonic ordering key (store-assigned). Wall-clock
    /// millis can tie when runs start within the same millisecond, so
    /// the store key orders on this strictly-increasing sequence
    /// instead.
    #[serde(default)]
    pub seq: u64,
    /// When the run started.
    pub started_at: DateTime<Utc>,
    /// When the run finished; `None` while running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    /// Current status.
    pub status: ProcessRunStatus,
    /// Short result summary on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Error detail on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Input for [`ProcessStore::create`].
#[derive(Clone, Deserialize)]
pub struct NewProcess {
    /// Human-readable name (required, bounded).
    pub name: String,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
    /// Cron expression (validated).
    pub cron: String,
    /// Instruction the agent executes (required, bounded).
    pub prompt: String,
    /// Optional structured config.
    #[serde(default)]
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
    /// Defaults to `true` when omitted.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

const fn default_enabled() -> bool {
    true
}

/// Partial update for [`ProcessStore::update`]. `None` fields keep the
/// stored value; `description` / `config` use a nested `Option` so an
/// explicit `null` clears them.
#[derive(Clone, Default, Deserialize)]
pub struct ProcessUpdate {
    /// New name, when present.
    #[serde(default)]
    pub name: Option<String>,
    /// New description; `Some(None)` clears it.
    #[serde(default, with = "double_option")]
    pub description: Option<Option<String>>,
    /// New cron expression, when present (re-validated).
    #[serde(default)]
    pub cron: Option<String>,
    /// New prompt, when present.
    #[serde(default)]
    pub prompt: Option<String>,
    /// New config; `Some(None)` clears it.
    #[serde(default, with = "double_option")]
    pub config: Option<Option<serde_json::Map<String, serde_json::Value>>>,
    /// New enabled flag, when present.
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// Serde helper distinguishing "field absent" (`None`) from "field
/// explicitly null" (`Some(None)`) on [`ProcessUpdate`].
mod double_option {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
    where
        T: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Option::<T>::deserialize(de).map(Some)
    }
}

/// The **only** process-derived data permitted to leave the agent VM
/// ("trigger outside, data inside").
///
/// Structurally contains no prompt, no config, and no run data — just
/// what an external cron service needs to fire a content-free trigger:
/// which process, on what schedule, whether it is active, and the next
/// computed fire time. The next phase exports this to the swarm
/// gateway's `process_triggers` CF; nothing else from this module may
/// cross the boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessTriggerMeta {
    /// Process id (opaque off-VM).
    pub process_id: String,
    /// Cron expression (UTC) — schedule only, no payload.
    pub cron: String,
    /// Whether the trigger should fire.
    pub enabled: bool,
    /// Next computed fire time, if the schedule has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<DateTime<Utc>>,
}
