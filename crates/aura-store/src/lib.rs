//! # aura-store
//!
//! `RocksDB` storage implementation for Aura.
//!
//! Provides:
//! - Column families for Record, Agent metadata, and Inbox
//! - Atomic commit protocol via `WriteBatch`
//! - Key encoding/decoding utilities

#![forbid(unsafe_code)]
#![warn(clippy::all)]

mod error;
mod keys;
mod rocks_store;
mod store;

pub use aura_core::AgentStatus;
pub use error::StoreError;
pub use keys::{AgentMetaKey, InboxKey, KeyCodec, MetaField, RecordKey};
#[cfg(any(test, feature = "test-support"))]
pub use rocks_store::FaultAt;
pub use rocks_store::RocksStore;
pub use store::{DequeueToken, ReadStore, Store, WriteStore};

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
    /// Skill installations per agent
    pub const AGENT_SKILLS: &str = "agent_skills";
    /// Persisted runtime capability ledger per agent
    pub const RUNTIME_CAPABILITIES: &str = "runtime_capabilities";
}
