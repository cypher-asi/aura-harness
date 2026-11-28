//! Errors of the agent store.

#[cfg(doc)]
use super::{derive_agent_id, AgentStore};
use crate::seal::SealError;

/// Errors produced by [`AgentStore`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentStoreError {
    /// A record's `agent_id` is not [`derive_agent_id`] of its `zns_id`.
    #[error("agent {agent_id}: id is not derived from its zns id (expected {expected})")]
    IdMismatch {
        /// Full hex of the record's `agent_id`.
        agent_id: String,
        /// Full hex of the id derived from its `zns_id`.
        expected: String,
    },
    /// A project id that can't be a key.
    #[error("invalid project id {0:?}: must be non-empty")]
    InvalidProjectId(String),
    /// The database was opened without a column family this store needs.
    #[error("{0} column family not found")]
    MissingColumnFamily(&'static str),
    /// A `RocksDB` read or write failed.
    #[error("{context}: {source}")]
    Backend {
        /// What was being read or written.
        context: String,
        /// The `RocksDB` failure.
        #[source]
        source: rocksdb::Error,
    },
    /// Sealing or unsealing a value failed.
    #[error("{context}: {source}")]
    Seal {
        /// Which value.
        context: String,
        /// The sealing failure.
        #[source]
        source: SealError,
    },
    /// A value failed to encode or decode as JSON.
    #[error("{context}: {source}")]
    Serde {
        /// Which value.
        context: String,
        /// The JSON failure.
        #[source]
        source: serde_json::Error,
    },
    /// A failure injected by [`AgentStore::apply_with_fault`].
    #[doc(hidden)]
    #[error("injected fault: {0:?}")]
    InjectedFault(crate::FaultAt),
}
