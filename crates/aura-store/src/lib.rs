//! # aura-store
//!
//! `RocksDB` storage implementation for Aura.
//!
//! Provides:
//! - Column families for Record, Agent metadata, and Inbox
//! - Atomic commit protocol via `WriteBatch`
//! - Key encoding/decoding utilities

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

mod error;
mod keys;
mod rocks_store;
mod store;

pub use aura_core::AgentStatus;
pub use error::StoreError;
pub use keys::{AgentMetaKey, InboxKey, KeyCodec, MetaField, RecordKey};
pub use rocks_store::RocksStore;
pub use store::{DequeueToken, Store};

/// Column family names.
pub mod cf {
    /// Record entries (append-only log per agent)
    pub const RECORD: &str = "record";
    /// Agent metadata (`head_seq`, status, etc.)
    pub const AGENT_META: &str = "agent_meta";
    /// Inbox (durable per-agent transaction queue)
    pub const INBOX: &str = "inbox";
}
