#![allow(deprecated)]
use super::*;
use proptest::prelude::*;

proptest! {
    #[test]
    fn proptest_agent_id_different_inputs_produce_different_ids(
        a in any::<[u8; 16]>(),
        b in any::<[u8; 16]>(),
    ) {
        let uuid_a = uuid::Uuid::from_bytes(a);
        let uuid_b = uuid::Uuid::from_bytes(b);
        let id_a = AgentId::from_uuid(uuid_a);
        let id_b = AgentId::from_uuid(uuid_b);
        if a == b {
            prop_assert_eq!(id_a, id_b);
        } else {
            prop_assert_ne!(id_a, id_b);
        }
    }

    #[test]
    fn proptest_tx_id_different_content_produces_different_ids(
        a in proptest::collection::vec(any::<u8>(), 1..256),
        b in proptest::collection::vec(any::<u8>(), 1..256),
    ) {
        let id_a = TxId::from_content(&a);
        let id_b = TxId::from_content(&b);
        if a == b {
            prop_assert_eq!(id_a, id_b);
        } else {
            prop_assert_ne!(id_a, id_b);
        }
    }

    #[test]
    fn proptest_action_id_hex_roundtrip(bytes in any::<[u8; 16]>()) {
        let id = ActionId::new(bytes);
        let hex = id.to_hex();
        let parsed = ActionId::from_hex(&hex).unwrap();
        prop_assert_eq!(id, parsed);
    }

    #[test]
    fn proptest_agent_id_hex_roundtrip(bytes in any::<[u8; 32]>()) {
        let id = AgentId::new(bytes);
        let hex = id.to_hex();
        let parsed = AgentId::from_hex(&hex).unwrap();
        prop_assert_eq!(id, parsed);
    }

    #[test]
    fn proptest_hash_hex_roundtrip(bytes in any::<[u8; 32]>()) {
        let hash = Hash::new(bytes);
        let hex = hash.to_hex();
        let parsed = Hash::from_hex(&hex).unwrap();
        prop_assert_eq!(hash, parsed);
    }

    #[test]
    fn proptest_process_id_hex_roundtrip(bytes in any::<[u8; 16]>()) {
        let id = ProcessId::new(bytes);
        let hex = id.to_hex();
        let parsed = ProcessId::from_hex(&hex).unwrap();
        prop_assert_eq!(id, parsed);
    }
}

#[test]
fn agent_id_generate_uniqueness() {
    let ids: Vec<AgentId> = (0..100).map(|_| AgentId::generate()).collect();
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            assert_ne!(ids[i], ids[j], "Generated IDs should be unique");
        }
    }
}

#[test]
fn action_id_generate_uniqueness() {
    let ids: Vec<ActionId> = (0..100).map(|_| ActionId::generate()).collect();
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            assert_ne!(ids[i], ids[j], "Generated IDs should be unique");
        }
    }
}

#[test]
fn process_id_generate_uniqueness() {
    let ids: Vec<ProcessId> = (0..100).map(|_| ProcessId::generate()).collect();
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            assert_ne!(ids[i], ids[j], "Generated IDs should be unique");
        }
    }
}

#[test]
fn hash_from_hex_invalid_length() {
    assert!(Hash::from_hex("abcd").is_err());
    assert!(Hash::from_hex("").is_err());
}

#[test]
fn hash_from_hex_invalid_chars() {
    let bad_hex = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
    assert!(Hash::from_hex(bad_hex).is_err());
}

#[test]
fn agent_id_display_and_debug() {
    let id = AgentId::new([0xAB; 32]);
    let display = format!("{id}");
    let debug = format!("{id:?}");
    assert!(display.len() == 16);
    assert!(debug.contains("AgentId("));
}

#[test]
fn hash_display_and_debug() {
    let hash = Hash::from_content(b"test");
    let display = format!("{hash}");
    let debug = format!("{hash:?}");
    assert!(display.len() == 16);
    assert!(debug.contains("Hash("));
}

#[test]
fn hash_genesis() {
    let content = b"genesis transaction";
    let hash1 = Hash::from_content(content);
    let hash2 = Hash::from_content(content);
    assert_eq!(hash1, hash2);

    // Genesis with chained method (None prev) should be same as from_content
    let hash3 = Hash::from_content_chained(content, None);
    assert_eq!(hash1, hash3);
}

#[test]
fn hash_chaining() {
    let content1 = b"first transaction";
    let content2 = b"second transaction";

    let hash1 = Hash::from_content(content1);
    let hash2 = Hash::from_content_chained(content2, Some(&hash1));

    // Same content with different prev_hash produces different hash
    let hash3 = Hash::from_content_chained(content2, None);
    assert_ne!(hash2, hash3);

    // Deterministic - same inputs produce same hash
    let hash4 = Hash::from_content_chained(content2, Some(&hash1));
    assert_eq!(hash2, hash4);
}

#[test]
fn hash_chain_integrity() {
    // Build a chain
    let h1 = Hash::from_content(b"tx1");
    let h2 = Hash::from_content_chained(b"tx2", Some(&h1));
    let h3 = Hash::from_content_chained(b"tx3", Some(&h2));

    // Verify chain - modify middle tx content should change downstream hashes
    let h2_modified = Hash::from_content_chained(b"tx2-modified", Some(&h1));
    assert_ne!(h2, h2_modified);

    let h3_from_modified = Hash::from_content_chained(b"tx3", Some(&h2_modified));
    assert_ne!(h3, h3_from_modified);
}

#[test]
fn hash_roundtrip() {
    let hash = Hash::from_content(b"test content");
    let hex = hash.to_hex();
    let parsed = Hash::from_hex(&hex).unwrap();
    assert_eq!(hash, parsed);
}

#[test]
fn hash_json_roundtrip() {
    let hash = Hash::from_content(b"test content");
    let json = serde_json::to_string(&hash).unwrap();
    let parsed: Hash = serde_json::from_str(&json).unwrap();
    assert_eq!(hash, parsed);
}

#[test]
fn agent_id_roundtrip() {
    let id = AgentId::generate();
    let hex = id.to_hex();
    let parsed = AgentId::from_hex(&hex).unwrap();
    assert_eq!(id, parsed);
}

#[test]
fn agent_id_json_roundtrip() {
    let id = AgentId::generate();
    let json = serde_json::to_string(&id).unwrap();
    let parsed: AgentId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, parsed);
}

#[test]
fn tx_id_from_content() {
    let content = b"test transaction content";
    let id1 = TxId::from_content(content);
    let id2 = TxId::from_content(content);
    assert_eq!(id1, id2);

    let id3 = TxId::from_content(b"different content");
    assert_ne!(id1, id3);
}

#[test]
fn action_id_roundtrip() {
    let id = ActionId::generate();
    let hex = id.to_hex();
    let parsed = ActionId::from_hex(&hex).unwrap();
    assert_eq!(id, parsed);
}

#[test]
fn action_id_json_roundtrip() {
    let id = ActionId::generate();
    let json = serde_json::to_string(&id).unwrap();
    let parsed: ActionId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, parsed);
}

#[test]
fn process_id_roundtrip() {
    let id = ProcessId::generate();
    let hex = id.to_hex();
    let parsed = ProcessId::from_hex(&hex).unwrap();
    assert_eq!(id, parsed);
}

#[test]
fn process_id_json_roundtrip() {
    let id = ProcessId::generate();
    let json = serde_json::to_string(&id).unwrap();
    let parsed: ProcessId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, parsed);
}
