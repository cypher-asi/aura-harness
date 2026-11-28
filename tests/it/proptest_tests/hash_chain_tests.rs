//! Hash chain property tests.

use super::*;
use aura_core_types::{Hash, ProcessId};

proptest! {
    /// Hash is deterministic - same content always produces same hash.
    #[test]
    fn prop_hash_deterministic(content: Vec<u8>) {
        let hash1 = Hash::from_content(&content);
        let hash2 = Hash::from_content(&content);
        prop_assert_eq!(hash1, hash2);
    }

    /// Hash with chaining is deterministic.
    #[test]
    fn prop_hash_chained_deterministic(content: Vec<u8>, prev_content: Vec<u8>) {
        let prev_hash = Hash::from_content(&prev_content);

        let hash1 = Hash::from_content_chained(&content, Some(&prev_hash));
        let hash2 = Hash::from_content_chained(&content, Some(&prev_hash));
        prop_assert_eq!(hash1, hash2);
    }

    /// Different chain orderings produce different hashes.
    #[test]
    fn prop_hash_chain_order_matters(content1: Vec<u8>, content2: Vec<u8>) {
        prop_assume!(!content1.is_empty() && !content2.is_empty());
        prop_assume!(content1 != content2);

        // Chain: content1 -> content2
        let hash1 = Hash::from_content(&content1);
        let chain1_final = Hash::from_content_chained(&content2, Some(&hash1));

        // Chain: content2 -> content1
        let hash2 = Hash::from_content(&content2);
        let chain2_final = Hash::from_content_chained(&content1, Some(&hash2));

        // Different orderings should produce different final hashes
        prop_assert_ne!(chain1_final, chain2_final);
    }

    /// Any content change changes the hash.
    #[test]
    fn prop_hash_content_sensitive(content1: Vec<u8>, content2: Vec<u8>) {
        prop_assume!(content1 != content2);

        let hash1 = Hash::from_content(&content1);
        let hash2 = Hash::from_content(&content2);
        prop_assert_ne!(hash1, hash2);
    }

    /// Chained hashes with different prev_hash produce different results.
    #[test]
    fn prop_hash_chain_depends_on_prev(content: Vec<u8>, prev1: Vec<u8>, prev2: Vec<u8>) {
        prop_assume!(prev1 != prev2);

        let prev_hash1 = Hash::from_content(&prev1);
        let prev_hash2 = Hash::from_content(&prev2);

        let chained1 = Hash::from_content_chained(&content, Some(&prev_hash1));
        let chained2 = Hash::from_content_chained(&content, Some(&prev_hash2));

        // Same content but different prev should produce different hash
        prop_assert_ne!(chained1, chained2);
    }

    /// Genesis (no prev) vs chained (with prev) produce different hashes.
    #[test]
    fn prop_genesis_differs_from_chained(content: Vec<u8>, prev_content: Vec<u8>) {
        let prev_hash = Hash::from_content(&prev_content);

        let genesis = Hash::from_content(&content);
        let chained = Hash::from_content_chained(&content, Some(&prev_hash));

        // Genesis and chained should differ (unless prev_hash happens to be all zeros,
        // but that's astronomically unlikely with random content)
        prop_assert_ne!(genesis, chained);
    }

    /// Hash hex roundtrip works for any content.
    #[test]
    fn prop_hash_hex_roundtrip(content: Vec<u8>) {
        let hash = Hash::from_content(&content);
        let hex = hash.to_hex();
        let parsed = Hash::from_hex(&hex).unwrap();
        prop_assert_eq!(hash, parsed);
    }

    /// ProcessId hex roundtrip works for any bytes.
    #[test]
    fn prop_process_id_hex_roundtrip(bytes: [u8; 16]) {
        let id = ProcessId::new(bytes);
        let hex = id.to_hex();
        let parsed = ProcessId::from_hex(&hex).unwrap();
        prop_assert_eq!(id, parsed);
    }
}
