//! ID roundtrip tests.

use super::*;
use aura_core_types::{ActionId, AgentId, Hash};

proptest! {
    /// AgentId roundtrips through hex encoding.
    #[test]
    fn agent_id_hex_roundtrip(bytes: [u8; 32]) {
        let id = AgentId::new(bytes);
        let hex = id.to_hex();
        let parsed = AgentId::from_hex(&hex).unwrap();
        prop_assert_eq!(id, parsed);
    }

    /// AgentId roundtrips through JSON serialization.
    #[test]
    fn agent_id_json_roundtrip(bytes: [u8; 32]) {
        let id = AgentId::new(bytes);
        let json = serde_json::to_string(&id).unwrap();
        let parsed: AgentId = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(id, parsed);
    }

    /// Hash from same content produces same hash.
    #[test]
    fn hash_from_content_deterministic(content: Vec<u8>) {
        let id1 = Hash::from_content(&content);
        let id2 = Hash::from_content(&content);
        prop_assert_eq!(id1, id2);
    }

    /// Hash from different content produces different hashes (with high probability).
    #[test]
    fn hash_from_content_different_content(content1: Vec<u8>, content2: Vec<u8>) {
        prop_assume!(content1 != content2);
        let id1 = Hash::from_content(&content1);
        let id2 = Hash::from_content(&content2);
        prop_assert_ne!(id1, id2);
    }

    /// ActionId roundtrips through hex encoding.
    #[test]
    fn action_id_hex_roundtrip(bytes: [u8; 16]) {
        let id = ActionId::new(bytes);
        let hex = id.to_hex();
        let parsed = ActionId::from_hex(&hex).unwrap();
        prop_assert_eq!(id, parsed);
    }
}
