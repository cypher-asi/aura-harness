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
//!   `inbox_tail`, `status`, `schema_version`).  Updated in-place.
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
    SchemaVersion = 4,
}

impl MetaField {
    /// Convert to byte representation.
    #[must_use]
    pub const fn as_byte(self) -> u8 {
        self as u8
    }

    /// Try to parse from byte.
    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::HeadSeq),
            1 => Some(Self::InboxHead),
            2 => Some(Self::InboxTail),
            3 => Some(Self::Status),
            4 => Some(Self::SchemaVersion),
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
            return Err(StoreError::InvalidKey("invalid agent meta key length".into()));
        }
        if bytes[0] != prefix::AGENT_META {
            return Err(StoreError::InvalidKey("invalid agent meta key prefix".into()));
        }

        let agent_bytes: [u8; 32] = bytes[1..33]
            .try_into()
            .map_err(|_| StoreError::InvalidKey("invalid agent_id bytes".into()))?;
        let field =
            MetaField::from_byte(bytes[33]).ok_or_else(|| StoreError::InvalidKey("invalid meta field".into()))?;

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
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn arb_agent_id() -> impl Strategy<Value = AgentId> {
        any::<[u8; 32]>().prop_map(AgentId::new)
    }

    fn arb_meta_field() -> impl Strategy<Value = MetaField> {
        prop_oneof![
            Just(MetaField::HeadSeq),
            Just(MetaField::InboxHead),
            Just(MetaField::InboxTail),
            Just(MetaField::Status),
            Just(MetaField::SchemaVersion),
        ]
    }

    proptest! {
        #[test]
        fn proptest_record_key_roundtrip(
            agent_id in arb_agent_id(),
            seq in any::<u64>(),
        ) {
            let key = RecordKey::new(agent_id, seq);
            let encoded = key.encode();
            let decoded = RecordKey::decode(&encoded).unwrap();
            prop_assert_eq!(key, decoded);
        }

        #[test]
        fn proptest_agent_meta_key_roundtrip(
            agent_id in arb_agent_id(),
            field in arb_meta_field(),
        ) {
            let key = AgentMetaKey::new(agent_id, field);
            let encoded = key.encode();
            let decoded = AgentMetaKey::decode(&encoded).unwrap();
            prop_assert_eq!(key, decoded);
        }

        #[test]
        fn proptest_inbox_key_roundtrip(
            agent_id in arb_agent_id(),
            inbox_seq in any::<u64>(),
        ) {
            let key = InboxKey::new(agent_id, inbox_seq);
            let encoded = key.encode();
            let decoded = InboxKey::decode(&encoded).unwrap();
            prop_assert_eq!(key, decoded);
        }

        #[test]
        fn proptest_record_key_ordering_preserved(
            agent_id in arb_agent_id(),
            seq_a in any::<u64>(),
            seq_b in any::<u64>(),
        ) {
            let key_a = RecordKey::new(agent_id, seq_a).encode();
            let key_b = RecordKey::new(agent_id, seq_b).encode();
            prop_assert_eq!(key_a.cmp(&key_b), seq_a.cmp(&seq_b));
        }

        #[test]
        fn proptest_inbox_key_ordering_preserved(
            agent_id in arb_agent_id(),
            seq_a in any::<u64>(),
            seq_b in any::<u64>(),
        ) {
            let key_a = InboxKey::new(agent_id, seq_a).encode();
            let key_b = InboxKey::new(agent_id, seq_b).encode();
            prop_assert_eq!(key_a.cmp(&key_b), seq_a.cmp(&seq_b));
        }
    }

    #[test]
    fn record_key_roundtrip() {
        let agent = AgentId::generate();
        let key = RecordKey::new(agent, 42);
        let encoded = key.encode();
        let decoded = RecordKey::decode(&encoded).unwrap();
        assert_eq!(key, decoded);
    }

    #[test]
    fn record_key_ordering() {
        let agent = AgentId::new([1u8; 32]);
        let key1 = RecordKey::new(agent, 1).encode();
        let key2 = RecordKey::new(agent, 2).encode();
        let key10 = RecordKey::new(agent, 10).encode();

        assert!(key1 < key2);
        assert!(key2 < key10);
    }

    #[test]
    fn agent_meta_key_roundtrip() {
        let agent = AgentId::generate();
        let key = AgentMetaKey::head_seq(agent);
        let encoded = key.encode();
        let decoded = AgentMetaKey::decode(&encoded).unwrap();
        assert_eq!(key, decoded);
    }

    #[test]
    fn inbox_key_roundtrip() {
        let agent = AgentId::generate();
        let key = InboxKey::new(agent, 100);
        let encoded = key.encode();
        let decoded = InboxKey::decode(&encoded).unwrap();
        assert_eq!(key, decoded);
    }

    #[test]
    fn inbox_key_ordering() {
        let agent = AgentId::new([2u8; 32]);
        let key1 = InboxKey::new(agent, 1).encode();
        let key2 = InboxKey::new(agent, 2).encode();

        assert!(key1 < key2);
    }

    // Edge case values: 0, u64::MAX, all-zero AgentId, all-FF AgentId

    #[test]
    fn record_key_seq_zero() {
        let agent = AgentId::new([0u8; 32]);
        let key = RecordKey::new(agent, 0);
        let encoded = key.encode();
        let decoded = RecordKey::decode(&encoded).unwrap();
        assert_eq!(key, decoded);
    }

    #[test]
    fn record_key_seq_max() {
        let agent = AgentId::new([0xFF; 32]);
        let key = RecordKey::new(agent, u64::MAX);
        let encoded = key.encode();
        let decoded = RecordKey::decode(&encoded).unwrap();
        assert_eq!(key, decoded);
        assert_eq!(decoded.seq, u64::MAX);
    }

    #[test]
    fn inbox_key_seq_zero_all_zero_agent() {
        let agent = AgentId::new([0u8; 32]);
        let key = InboxKey::new(agent, 0);
        let encoded = key.encode();
        let decoded = InboxKey::decode(&encoded).unwrap();
        assert_eq!(key, decoded);
    }

    #[test]
    fn inbox_key_seq_max_all_ff_agent() {
        let agent = AgentId::new([0xFF; 32]);
        let key = InboxKey::new(agent, u64::MAX);
        let encoded = key.encode();
        let decoded = InboxKey::decode(&encoded).unwrap();
        assert_eq!(key, decoded);
        assert_eq!(decoded.inbox_seq, u64::MAX);
    }

    #[test]
    fn agent_meta_key_all_zero_agent() {
        let agent = AgentId::new([0u8; 32]);
        for field in [
            MetaField::HeadSeq,
            MetaField::InboxHead,
            MetaField::InboxTail,
            MetaField::Status,
            MetaField::SchemaVersion,
        ] {
            let key = AgentMetaKey::new(agent, field);
            let encoded = key.encode();
            let decoded = AgentMetaKey::decode(&encoded).unwrap();
            assert_eq!(key, decoded);
        }
    }

    #[test]
    fn agent_meta_key_all_ff_agent() {
        let agent = AgentId::new([0xFF; 32]);
        for field in [
            MetaField::HeadSeq,
            MetaField::InboxHead,
            MetaField::InboxTail,
            MetaField::Status,
            MetaField::SchemaVersion,
        ] {
            let key = AgentMetaKey::new(agent, field);
            let encoded = key.encode();
            let decoded = AgentMetaKey::decode(&encoded).unwrap();
            assert_eq!(key, decoded);
        }
    }

    #[test]
    fn record_key_decode_wrong_length() {
        assert!(RecordKey::decode(&[]).is_err());
        assert!(RecordKey::decode(&[prefix::RECORD]).is_err());
        assert!(RecordKey::decode(&[prefix::RECORD; 100]).is_err());
    }

    #[test]
    fn record_key_decode_wrong_prefix() {
        let agent = AgentId::new([1u8; 32]);
        let mut encoded = RecordKey::new(agent, 1).encode();
        encoded[0] = b'X';
        assert!(RecordKey::decode(&encoded).is_err());
    }

    #[test]
    fn inbox_key_decode_wrong_length() {
        assert!(InboxKey::decode(&[]).is_err());
        assert!(InboxKey::decode(&[prefix::INBOX; 2]).is_err());
    }

    #[test]
    fn inbox_key_decode_wrong_prefix() {
        let agent = AgentId::new([1u8; 32]);
        let mut encoded = InboxKey::new(agent, 1).encode();
        encoded[0] = b'X';
        assert!(InboxKey::decode(&encoded).is_err());
    }

    #[test]
    fn agent_meta_key_decode_wrong_length() {
        assert!(AgentMetaKey::decode(&[]).is_err());
        assert!(AgentMetaKey::decode(&[prefix::AGENT_META; 2]).is_err());
    }

    #[test]
    fn agent_meta_key_decode_invalid_field() {
        let agent = AgentId::new([1u8; 32]);
        let mut encoded = AgentMetaKey::head_seq(agent).encode();
        encoded[33] = 0xFF; // Invalid field discriminant
        assert!(AgentMetaKey::decode(&encoded).is_err());
    }

    #[test]
    fn meta_field_byte_roundtrip() {
        for field in [
            MetaField::HeadSeq,
            MetaField::InboxHead,
            MetaField::InboxTail,
            MetaField::Status,
            MetaField::SchemaVersion,
        ] {
            let byte = field.as_byte();
            let parsed = MetaField::from_byte(byte).unwrap();
            assert_eq!(field, parsed);
        }
    }

    #[test]
    fn meta_field_from_invalid_byte() {
        assert!(MetaField::from_byte(5).is_none());
        assert!(MetaField::from_byte(255).is_none());
    }
}
