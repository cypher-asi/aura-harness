//! Hash tests.

use super::*;
use aura_core_types::hash;

proptest! {
    /// Hashing is deterministic.
    #[test]
    fn hash_deterministic(data: Vec<u8>) {
        let hash1 = hash::hash_bytes(&data);
        let hash2 = hash::hash_bytes(&data);
        prop_assert_eq!(hash1, hash2);
    }

    /// Different data produces different hashes (with high probability).
    #[test]
    fn hash_different_data(data1: Vec<u8>, data2: Vec<u8>) {
        prop_assume!(data1 != data2);
        let hash1 = hash::hash_bytes(&data1);
        let hash2 = hash::hash_bytes(&data2);
        prop_assert_ne!(hash1, hash2);
    }

    /// Hash order matters for hash_many.
    #[test]
    fn hash_many_order_matters(part1: Vec<u8>, part2: Vec<u8>) {
        // Both parts must be non-empty and different for order to matter
        prop_assume!(part1 != part2 && !part1.is_empty() && !part2.is_empty());
        let hash1 = hash::hash_many(&[&part1, &part2]);
        let hash2 = hash::hash_many(&[&part2, &part1]);
        prop_assert_ne!(hash1, hash2);
    }

    /// Incremental hasher produces same result as hash_many.
    #[test]
    fn incremental_hasher_equivalent(parts: Vec<Vec<u8>>) {
        let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        let direct = hash::hash_many(&refs);

        let mut hasher = hash::Hasher::new();
        for part in &parts {
            hasher.update(part);
        }
        let incremental = hasher.finalize();

        prop_assert_eq!(direct, incremental);
    }
}
