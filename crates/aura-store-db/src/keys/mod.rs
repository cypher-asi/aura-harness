//! Storage key encoding and decoding for `RocksDB`.
//!
//! # Key Format
//!
//! Every key starts with a single-byte prefix that identifies the column family
//! it belongs to, followed by a 32-byte `AgentId`, then a type-specific suffix:
//!
//! | Column   | Prefix | Layout                                    | Size    |
//! |----------|--------|-------------------------------------------|---------|
//! | Record   | `R`    | `R` · `agent_id[32]` · `seq[u64be]`      | 41 B    |
//! | Metadata | `M`    | `M` · `agent_id[32]` · `field[u8]`       | 34 B    |
//! | Inbox    | `Q`    | `Q` · `agent_id[32]` · `inbox_seq[u64be]`| 41 B    |
//!
//! # Ordering Guarantees
//!
//! All integer fields use **big-endian** encoding so that `RocksDB`'s default
//! byte-wise comparator produces ascending numeric order.  This means:
//!
//! - Record entries for a given agent are physically sorted by `seq`.
//! - Inbox entries for a given agent are physically sorted by `inbox_seq`.
//! - A prefix scan with `agent_id` returns entries in sequence order.
//!
//! # Column Family Semantics
//!
//! - **Record** (`R`): Append-only log of `RecordEntry` values, keyed by
//!   `(agent_id, seq)`.  Entries are never deleted.
//! - **Metadata** (`M`): Per-agent scalars (`head_seq`, `inbox_head`,
//!   `inbox_tail`, `status`, `processing_claim`, `schema_version`). Updated
//!   in-place.
//! - **Inbox** (`Q`): FIFO queue of pending `Transaction` values.  Entries are
//!   deleted after being committed to the record via `append_entry_atomic`.
//!
//! # Failure Modes
//!
//! `KeyCodec::decode` returns `StoreError::InvalidKey` when the byte slice has
//! the wrong length, an unrecognised prefix byte, or an unknown metadata field
//! discriminant.

use aura_core::AgentId;

use crate::error::StoreError;

/// Key prefix bytes.
pub mod prefix {
    /// Record entries: `R | agent_id(32) | seq(u64be)`
    pub const RECORD: u8 = b'R';
    /// Agent metadata: `M | agent_id(32) | field`
    pub const AGENT_META: u8 = b'M';
    /// Inbox: `Q | agent_id(32) | inbox_seq(u64be)`
    pub const INBOX: u8 = b'Q';
}

/// Metadata field identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MetaField {
    /// Head sequence number
    HeadSeq = 0,
    /// Inbox head cursor
    InboxHead = 1,
    /// Inbox tail cursor
    InboxTail = 2,
    /// Agent status
    Status = 3,
    /// Schema version
    #[deprecated(note = "reserved for future use")]
    SchemaVersion = 4,
    /// Store-backed single-processing claim
    ProcessingClaim = 5,
}

impl MetaField {
    /// Convert to byte representation.
    #[must_use]
    pub const fn as_byte(self) -> u8 {
        self as u8
    }

    /// Try to parse from byte.
    #[must_use]
    #[allow(deprecated)]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::HeadSeq),
            1 => Some(Self::InboxHead),
            2 => Some(Self::InboxTail),
            3 => Some(Self::Status),
            4 => Some(Self::SchemaVersion),
            5 => Some(Self::ProcessingClaim),
            _ => None,
        }
    }
}

/// Trait for key encoding/decoding.
pub trait KeyCodec: Sized {
    /// Encode to bytes.
    fn encode(&self) -> Vec<u8>;

    /// Decode from bytes.
    ///
    /// # Errors
    /// Returns `StoreError::InvalidKey` if bytes don't represent a valid key.
    fn decode(bytes: &[u8]) -> Result<Self, StoreError>;
}

/// Record key: `R | agent_id(32) | seq(u64be)`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordKey {
    pub agent_id: AgentId,
    pub seq: u64,
}

impl RecordKey {
    /// Create a new record key.
    #[must_use]
    pub const fn new(agent_id: AgentId, seq: u64) -> Self {
        Self { agent_id, seq }
    }

    /// Create the start key for scanning an agent's records.
    #[cfg(test)]
    #[must_use]
    pub fn scan_start(agent_id: AgentId) -> Vec<u8> {
        Self::new(agent_id, 0).encode()
    }

    /// Create the end key for scanning an agent's records (exclusive).
    #[must_use]
    pub fn scan_end(agent_id: AgentId) -> Vec<u8> {
        Self::new(agent_id, u64::MAX).encode()
    }

    /// Create a key for scanning from a specific sequence.
    #[must_use]
    pub fn scan_from(agent_id: AgentId, from_seq: u64) -> Vec<u8> {
        Self::new(agent_id, from_seq).encode()
    }
}

impl KeyCodec for RecordKey {
    fn encode(&self) -> Vec<u8> {
        let mut key = Vec::with_capacity(1 + 32 + 8);
        key.push(prefix::RECORD);
        key.extend_from_slice(self.agent_id.as_bytes());
        key.extend_from_slice(&self.seq.to_be_bytes());
        key
    }

    fn decode(bytes: &[u8]) -> Result<Self, StoreError> {
        if bytes.len() != 1 + 32 + 8 {
            return Err(StoreError::InvalidKey("invalid record key length".into()));
        }
        if bytes[0] != prefix::RECORD {
            return Err(StoreError::InvalidKey("invalid record key prefix".into()));
        }

        let agent_bytes: [u8; 32] = bytes[1..33]
            .try_into()
            .map_err(|_| StoreError::InvalidKey("invalid agent_id bytes".into()))?;
        let seq_bytes: [u8; 8] = bytes[33..41]
            .try_into()
            .map_err(|_| StoreError::InvalidKey("invalid seq bytes".into()))?;

        Ok(Self {
            agent_id: AgentId::new(agent_bytes),
            seq: u64::from_be_bytes(seq_bytes),
        })
    }
}

/// Agent metadata key: `M | agent_id(32) | field`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMetaKey {
    pub agent_id: AgentId,
    pub field: MetaField,
}

impl AgentMetaKey {
    /// Create a new agent metadata key.
    #[must_use]
    pub const fn new(agent_id: AgentId, field: MetaField) -> Self {
        Self { agent_id, field }
    }

    /// Create a `head_seq` key.
    #[must_use]
    pub const fn head_seq(agent_id: AgentId) -> Self {
        Self::new(agent_id, MetaField::HeadSeq)
    }

    /// Create an `inbox_head` key.
    #[must_use]
    pub const fn inbox_head(agent_id: AgentId) -> Self {
        Self::new(agent_id, MetaField::InboxHead)
    }

    /// Create an `inbox_tail` key.
    #[must_use]
    pub const fn inbox_tail(agent_id: AgentId) -> Self {
        Self::new(agent_id, MetaField::InboxTail)
    }

    /// Create a status key.
    #[must_use]
    pub const fn status(agent_id: AgentId) -> Self {
        Self::new(agent_id, MetaField::Status)
    }

    /// Create a processing-claim key.
    #[must_use]
    pub const fn processing_claim(agent_id: AgentId) -> Self {
        Self::new(agent_id, MetaField::ProcessingClaim)
    }
}

impl KeyCodec for AgentMetaKey {
    fn encode(&self) -> Vec<u8> {
        let mut key = Vec::with_capacity(1 + 32 + 1);
        key.push(prefix::AGENT_META);
        key.extend_from_slice(self.agent_id.as_bytes());
        key.push(self.field.as_byte());
        key
    }

    fn decode(bytes: &[u8]) -> Result<Self, StoreError> {
        if bytes.len() != 1 + 32 + 1 {
            return Err(StoreError::InvalidKey(
                "invalid agent meta key length".into(),
            ));
        }
        if bytes[0] != prefix::AGENT_META {
            return Err(StoreError::InvalidKey(
                "invalid agent meta key prefix".into(),
            ));
        }

        let agent_bytes: [u8; 32] = bytes[1..33]
            .try_into()
            .map_err(|_| StoreError::InvalidKey("invalid agent_id bytes".into()))?;
        let field = MetaField::from_byte(bytes[33])
            .ok_or_else(|| StoreError::InvalidKey("invalid meta field".into()))?;

        Ok(Self {
            agent_id: AgentId::new(agent_bytes),
            field,
        })
    }
}

/// Inbox key: `Q | agent_id(32) | inbox_seq(u64be)`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxKey {
    pub agent_id: AgentId,
    pub inbox_seq: u64,
}

impl InboxKey {
    /// Create a new inbox key.
    #[must_use]
    pub const fn new(agent_id: AgentId, inbox_seq: u64) -> Self {
        Self {
            agent_id,
            inbox_seq,
        }
    }

    /// Create the start key for scanning an agent's inbox.
    #[cfg(test)]
    #[must_use]
    pub fn scan_start(agent_id: AgentId) -> Vec<u8> {
        Self::new(agent_id, 0).encode()
    }

    /// Create the end key for scanning an agent's inbox (exclusive).
    #[cfg(test)]
    #[must_use]
    pub fn scan_end(agent_id: AgentId) -> Vec<u8> {
        Self::new(agent_id, u64::MAX).encode()
    }
}

impl KeyCodec for InboxKey {
    fn encode(&self) -> Vec<u8> {
        let mut key = Vec::with_capacity(1 + 32 + 8);
        key.push(prefix::INBOX);
        key.extend_from_slice(self.agent_id.as_bytes());
        key.extend_from_slice(&self.inbox_seq.to_be_bytes());
        key
    }

    fn decode(bytes: &[u8]) -> Result<Self, StoreError> {
        if bytes.len() != 1 + 32 + 8 {
            return Err(StoreError::InvalidKey("invalid inbox key length".into()));
        }
        if bytes[0] != prefix::INBOX {
            return Err(StoreError::InvalidKey("invalid inbox key prefix".into()));
        }

        let agent_bytes: [u8; 32] = bytes[1..33]
            .try_into()
            .map_err(|_| StoreError::InvalidKey("invalid agent_id bytes".into()))?;
        let seq_bytes: [u8; 8] = bytes[33..41]
            .try_into()
            .map_err(|_| StoreError::InvalidKey("invalid inbox_seq bytes".into()))?;

        Ok(Self {
            agent_id: AgentId::new(agent_bytes),
            inbox_seq: u64::from_be_bytes(seq_bytes),
        })
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests;
