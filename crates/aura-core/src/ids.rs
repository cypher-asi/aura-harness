//! Strongly-typed identifiers for the Aura system.
//!
//! All IDs are fixed-size byte arrays with display formatting and serialization.

use serde::{Deserialize, Serialize};
use std::fmt;

// ============================================================================
// Hash Type (32 bytes, blake3)
// ============================================================================

/// A 32-byte blake3 hash used for transaction chaining.
///
/// The hash is computed from content + previous hash, creating an immutable chain.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hash(#[serde(with = "crate::serde_helpers::hex_bytes_32")] pub [u8; 32]);

impl Hash {
    /// Create a new `Hash` from raw bytes.
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Create hash from content only (genesis transaction).
    #[must_use]
    pub fn from_content(content: &[u8]) -> Self {
        let hash = blake3::hash(content);
        Self(*hash.as_bytes())
    }

    /// Create hash from content and previous transaction's hash.
    /// Genesis transaction passes `None` for `prev_hash`.
    #[must_use]
    pub fn from_content_chained(content: &[u8], prev_hash: Option<&Self>) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(content);
        if let Some(prev) = prev_hash {
            hasher.update(&prev.0);
        }
        Self(*hasher.finalize().as_bytes())
    }

    /// Get the raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Convert to hex string.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse from hex string.
    ///
    /// # Errors
    /// Returns error if hex string is invalid or wrong length.
    pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
        let bytes = hex::decode(s)?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| hex::FromHexError::InvalidStringLength)?;
        Ok(Self(arr))
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({})", &self.to_hex()[..16])
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", &self.to_hex()[..16])
    }
}

// ============================================================================
// Agent ID (32 bytes)
// ============================================================================

/// Agent identifier - 32 bytes, derived from identity hash or UUID.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(#[serde(with = "crate::serde_helpers::hex_bytes_32")] pub [u8; 32]);

impl AgentId {
    /// Create a new `AgentId` from raw bytes.
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Create an `AgentId` from a UUID v4.
    #[must_use]
    pub fn from_uuid(uuid: uuid::Uuid) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(uuid.as_bytes());
        let hash = hasher.finalize();
        Self(*hash.as_bytes())
    }

    /// Generate a new random `AgentId`.
    #[must_use]
    pub fn generate() -> Self {
        Self::from_uuid(uuid::Uuid::new_v4())
    }

    /// Get the raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Convert to hex string.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse from hex string.
    ///
    /// # Errors
    /// Returns error if hex string is invalid or wrong length.
    pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
        let bytes = hex::decode(s)?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| hex::FromHexError::InvalidStringLength)?;
        Ok(Self(arr))
    }
}

impl fmt::Debug for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AgentId({})", &self.to_hex()[..16])
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", &self.to_hex()[..16])
    }
}

/// Transaction identifier - 32 bytes, typically a hash of tx content.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TxId(#[serde(with = "crate::serde_helpers::hex_bytes_32")] pub [u8; 32]);

impl TxId {
    /// Create a new `TxId` from raw bytes.
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Generate a `TxId` by hashing content.
    #[must_use]
    pub fn from_content(content: &[u8]) -> Self {
        let hash = blake3::hash(content);
        Self(*hash.as_bytes())
    }

    /// Get the raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Convert to hex string.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse from hex string.
    ///
    /// # Errors
    /// Returns error if hex string is invalid or wrong length.
    pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
        let bytes = hex::decode(s)?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| hex::FromHexError::InvalidStringLength)?;
        Ok(Self(arr))
    }
}

impl fmt::Debug for TxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TxId({})", &self.to_hex()[..16])
    }
}

impl fmt::Display for TxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", &self.to_hex()[..16])
    }
}

/// Action identifier - 16 bytes, generated per action.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActionId(#[serde(with = "crate::serde_helpers::hex_bytes_16")] pub [u8; 16]);

impl ActionId {
    /// Create a new `ActionId` from raw bytes.
    #[must_use]
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Generate a new random `ActionId`.
    #[must_use]
    pub fn generate() -> Self {
        let uuid = uuid::Uuid::new_v4();
        Self(*uuid.as_bytes())
    }

    /// Get the raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Convert to hex string.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse from hex string.
    ///
    /// # Errors
    /// Returns error if hex string is invalid or wrong length.
    pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
        let bytes = hex::decode(s)?;
        let arr: [u8; 16] = bytes
            .try_into()
            .map_err(|_| hex::FromHexError::InvalidStringLength)?;
        Ok(Self(arr))
    }
}

impl fmt::Debug for ActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ActionId({})", self.to_hex())
    }
}

impl fmt::Display for ActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

// ============================================================================
// Process ID (16 bytes)
// ============================================================================

/// Process identifier - 16 bytes, generated per async process.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessId(#[serde(with = "crate::serde_helpers::hex_bytes_16")] pub [u8; 16]);

impl ProcessId {
    /// Create a new `ProcessId` from raw bytes.
    #[must_use]
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Generate a new random `ProcessId`.
    #[must_use]
    pub fn generate() -> Self {
        let uuid = uuid::Uuid::new_v4();
        Self(*uuid.as_bytes())
    }

    /// Get the raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Convert to hex string.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse from hex string.
    ///
    /// # Errors
    /// Returns error if hex string is invalid or wrong length.
    pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
        let bytes = hex::decode(s)?;
        let arr: [u8; 16] = bytes
            .try_into()
            .map_err(|_| hex::FromHexError::InvalidStringLength)?;
        Ok(Self(arr))
    }
}

impl fmt::Debug for ProcessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcessId({})", self.to_hex())
    }
}

impl fmt::Display for ProcessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

// Re-export hex for crate-internal convenience
pub(crate) use hex;

#[cfg(test)]
mod tests {
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
            if a != b {
                prop_assert_ne!(id_a, id_b);
            } else {
                prop_assert_eq!(id_a, id_b);
            }
        }

        #[test]
        fn proptest_tx_id_different_content_produces_different_ids(
            a in proptest::collection::vec(any::<u8>(), 1..256),
            b in proptest::collection::vec(any::<u8>(), 1..256),
        ) {
            let id_a = TxId::from_content(&a);
            let id_b = TxId::from_content(&b);
            if a != b {
                prop_assert_ne!(id_a, id_b);
            } else {
                prop_assert_eq!(id_a, id_b);
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
}
