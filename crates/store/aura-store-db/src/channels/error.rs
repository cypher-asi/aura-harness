//! The error type of [`ChannelStore`](super::ChannelStore).

use crate::agents::AgentStoreError;
use crate::seal::SealError;

/// Errors produced by [`ChannelStore`](super::ChannelStore).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ChannelError {
    /// A slug that is not 1 to 40 characters of `a-z`, `0-9` and `-`.
    #[error("invalid channel slug {0:?}: must be 1-40 characters of a-z, 0-9 and -")]
    InvalidSlug(String),
    /// Another channel already has the slug.
    #[error("channel slug {0:?} is taken")]
    SlugTaken(String),
    /// No channel has the id.
    #[error("unknown channel {0}")]
    UnknownChannel(String),
    /// The agent is already a member of a channel.
    #[error("agent {agent} is already a member of channel {channel}")]
    AlreadyMember {
        /// Full hex of the agent id.
        agent: String,
        /// Id of the channel the agent is in.
        channel: String,
    },
    /// A channel id that can't be a key: empty or containing `/`.
    #[error("invalid channel id {0:?}: must be non-empty and contain no '/'")]
    InvalidId(String),
    /// A project id that is empty or all whitespace.
    #[error("invalid project id {0:?}: must not be empty or blank")]
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
    /// Staging the coordinator's agent row failed.
    #[error("staging coordinator agent row: {0}")]
    Agent(#[from] AgentStoreError),
    /// A failure injected by
    /// [`ChannelStore::create_with_fault`](super::ChannelStore::create_with_fault).
    #[cfg(any(test, feature = "test-support"))]
    #[error("injected fault: {0:?}")]
    InjectedFault(crate::FaultAt),
}
