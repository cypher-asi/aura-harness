//! Key encoding tests.

use super::*;
use aura_core_types::AgentId;
use aura_store_db::{AgentMetaKey, InboxKey, KeyCodec, MetaField, RecordKey};

proptest! {
    /// RecordKey roundtrips through encoding.
    #[test]
    fn record_key_roundtrip(agent_bytes: [u8; 32], seq: u64) {
        let agent_id = AgentId::new(agent_bytes);
        let key = RecordKey::new(agent_id, seq);
        let encoded = key.encode();
        let decoded = RecordKey::decode(&encoded).unwrap();
        prop_assert_eq!(key, decoded);
    }

    /// RecordKey encoding preserves ordering.
    #[test]
    fn record_key_ordering(agent_bytes: [u8; 32], seq1: u64, seq2: u64) {
        let agent_id = AgentId::new(agent_bytes);
        let key1 = RecordKey::new(agent_id, seq1).encode();
        let key2 = RecordKey::new(agent_id, seq2).encode();

        if seq1 < seq2 {
            prop_assert!(key1 < key2);
        } else if seq1 > seq2 {
            prop_assert!(key1 > key2);
        } else {
            prop_assert_eq!(key1, key2);
        }
    }

    /// InboxKey roundtrips through encoding.
    #[test]
    fn inbox_key_roundtrip(agent_bytes: [u8; 32], seq: u64) {
        let agent_id = AgentId::new(agent_bytes);
        let key = InboxKey::new(agent_id, seq);
        let encoded = key.encode();
        let decoded = InboxKey::decode(&encoded).unwrap();
        prop_assert_eq!(key, decoded);
    }

    /// InboxKey encoding preserves ordering.
    #[test]
    fn inbox_key_ordering(agent_bytes: [u8; 32], seq1: u64, seq2: u64) {
        let agent_id = AgentId::new(agent_bytes);
        let key1 = InboxKey::new(agent_id, seq1).encode();
        let key2 = InboxKey::new(agent_id, seq2).encode();

        if seq1 < seq2 {
            prop_assert!(key1 < key2);
        } else if seq1 > seq2 {
            prop_assert!(key1 > key2);
        } else {
            prop_assert_eq!(key1, key2);
        }
    }
}

#[test]
fn agent_meta_key_roundtrip_all_fields() {
    let agent_id = AgentId::generate();

    for field in [
        MetaField::HeadSeq,
        MetaField::InboxHead,
        MetaField::InboxTail,
        MetaField::Status,
        MetaField::ProcessingClaim,
    ] {
        let key = AgentMetaKey::new(agent_id, field);
        let encoded = key.encode();
        let decoded = AgentMetaKey::decode(&encoded).unwrap();
        assert_eq!(key, decoded);
    }
}
