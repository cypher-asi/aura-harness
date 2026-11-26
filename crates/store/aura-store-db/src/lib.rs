//! # aura-store-db
//!
//! Layer: store
//!
//! `RocksDB`-backed durable storage implementation for Aura.
//!
//! Phase 2 split: this crate is the renamed body of the legacy
//! `aura-store` crate. The original `aura-store` survives as a
//! re-export shell so existing call sites keep compiling.
//!
//! Provides:
//! - Column families for Record, Agent metadata, and Inbox
//! - Atomic commit protocol via `WriteBatch`
//! - Key encoding/decoding utilities
//! - A bridge from [`RocksStore`] to the
//!   [`aura_store_record::RecordLog`] trait for layered consumers.

#![forbid(unsafe_code)]
#![deny(clippy::too_many_lines)]

pub mod agents;
pub mod channels;
mod error;
mod keys;
pub mod migrate;
pub mod processes;
mod record_log_bridge;
mod rocks_store;
pub mod seal;
mod store;
pub mod traces;
pub mod vault;

pub use agents::{
    AgentRecord, AgentStore, AgentStoreError, ProjectRecord, RegistryDiff, RegistryMeta,
    RegistrySnapshot,
};
pub use aura_core_types::AgentStatus;
pub use channels::{
    valid_slug, ChannelError, ChannelPost, ChannelRecord, ChannelStore, ChannelUpdate,
    MemberRecord, MemberRole, NewChannel, NewPost, PostAuthor, ProjectLink,
};
pub use error::StoreError;
pub use keys::{AgentMetaKey, InboxKey, KeyCodec, MetaField, RecordKey};
pub use migrate::{seal_db_copy, SealCopyStats, SEALED_VALUE_CFS};
pub use processes::{
    NewProcess, ProcessError, ProcessRecord, ProcessRunRecord, ProcessRunStatus, ProcessStore,
    ProcessTriggerMeta, ProcessUpdate,
};
#[cfg(any(test, feature = "test-support"))]
pub use rocks_store::FaultAt;
pub use rocks_store::RocksStore;
/// Re-exported so callers can stage rows of several stores (e.g.
/// [`AgentStore::stage`]) into one atomic batch.
pub use rocksdb::WriteBatch;
pub use seal::{SealCipher, SealError, SEAL_MAGIC, SEAL_VERSION};
pub use store::{DequeueToken, ReadStore, Store, WriteStore};
pub use traces::{TraceStore, TraceStoreError};
pub use vault::{SecretMetadata, SecretRecord, SecretsVault, VaultError};

/// Column family names.
pub mod cf {
    /// Record entries (append-only log per agent)
    pub const RECORD: &str = "record";
    /// Agent metadata (`head_seq`, status, etc.)
    pub const AGENT_META: &str = "agent_meta";
    /// Inbox (durable per-agent transaction queue)
    pub const INBOX: &str = "inbox";
    /// Memory: per-agent semantic facts
    pub const MEMORY_FACTS: &str = "memory_facts";
    /// Memory: per-agent episodic events
    pub const MEMORY_EVENTS: &str = "memory_events";
    /// Memory: per-agent procedural patterns
    pub const MEMORY_PROCEDURES: &str = "memory_procedures";
    /// Memory: event ID → timestamp secondary index
    pub const MEMORY_EVENT_INDEX: &str = "memory_event_index";
    /// Memory: per-agent Agent Continuity configuration
    pub const MEMORY_CONFIG: &str = "memory_config";
    /// Skill installations per agent
    pub const AGENT_SKILLS: &str = "agent_skills";
    /// Persisted runtime capability ledger per agent
    pub const RUNTIME_CAPABILITIES: &str = "runtime_capabilities";
    /// Per-user tool-permission default policy (full_access /
    /// auto_review / default_permissions). Keyed by `user_id` bytes;
    /// value is a JSON-serialised [`aura_core_types::UserToolDefaults`].
    pub const USER_TOOL_DEFAULTS: &str = "user_tool_defaults";
    /// In-TEE secrets vault (Swarm TEE phase 6). Keyed by secret name;
    /// value is a JSON-serialised [`crate::vault::SecretRecord`],
    /// sealed at rest in sealed mode.
    pub const SECRETS: &str = "secrets";
    /// Process definitions (Swarm TEE phase 7). Keyed by process id;
    /// value is a JSON-serialised [`crate::processes::ProcessRecord`]
    /// (prompt + config included), sealed at rest in sealed mode.
    pub const PROCESSES: &str = "processes";
    /// Process run history (Swarm TEE phase 7). Keyed
    /// `{process_id}/{started_millis}/{run_id}`; value is a
    /// JSON-serialised [`crate::processes::ProcessRunRecord`], sealed
    /// at rest in sealed mode. Capped per process — see
    /// [`crate::processes::MAX_PROCESS_RUNS_KEPT`].
    pub const PROCESS_RUNS: &str = "process_runs";
    /// Run trace metadata and scorecards. Keyed `meta/{run_id}` and
    /// `summary/{run_id}`; values are JSON documents from
    /// `aura-agent-trace`, sealed at rest in sealed mode. See
    /// [`crate::traces`].
    pub const TRACE_RUNS: &str = "trace_runs";
    /// Run trace steps, rewards and kernel record rows. Keyed
    /// `{run_id}/{kind}/{n:020}`; sealed at rest in sealed mode.
    pub const TRACE_STEPS: &str = "trace_steps";
    /// Runs of each owner (agent id in hex). Keyed `{owner}/{run_id}`
    /// with an empty value; keys only, plaintext in both modes.
    pub const TRACE_INDEX: &str = "trace_index";
    /// Agents of the terminal registry. Keyed by the 32 raw bytes of
    /// the agent id (blake3 of its ZNS id); value is a JSON-serialised
    /// [`crate::agents::AgentRecord`], sealed at rest in sealed mode.
    /// See [`crate::agents`].
    pub const AGENTS: &str = "agents";
    /// Projects of the terminal registry. Keyed by the project id
    /// (utf-8); value is a JSON-serialised
    /// [`crate::agents::ProjectRecord`], sealed at rest in sealed mode.
    pub const PROJECTS: &str = "projects";
    /// Registry-wide settings. Keys `version`, `active`, `prefixes` and
    /// `last_numbers`; values are JSON, sealed at rest in sealed mode.
    /// See [`crate::agents::RegistryMeta`].
    pub const REGISTRY_META: &str = "registry_meta";
    /// Channels of agents. Keys `c/{channel_id}` (a JSON
    /// [`crate::channels::ChannelRecord`]), `s/{slug}` (the channel id
    /// owning the slug) and `p/{channel_id}/{project_id}` (a JSON
    /// [`crate::channels::ProjectLink`]); values sealed at rest in
    /// sealed mode. See [`crate::channels`].
    pub const CHANNELS: &str = "channels";
    /// Channel membership. Keys `m/{channel_id}/` + the 32 raw agent id
    /// bytes (a JSON [`crate::channels::MemberRecord`]) and `a/` + the 32
    /// raw agent id bytes (the channel id the agent is in); values
    /// sealed at rest in sealed mode.
    pub const CHANNEL_MEMBERS: &str = "channel_members";
    /// Channel posts. Keyed `{channel_id}/{at_ms:020}/{seq:010}`; value
    /// is a JSON [`crate::channels::ChannelPost`], sealed at rest in
    /// sealed mode.
    pub const CHANNEL_POSTS: &str = "channel_posts";
}
