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
pub struct Hash(#[serde(with = "hex_bytes_32")] pub [u8; 32]);

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
    /// Genesis transaction passes None for prev_hash.
    #[must_use]
    pub fn from_content_chained(content: &[u8], prev_hash: Option<&Hash>) -> Self {
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
pub struct AgentId(#[serde(with = "hex_bytes_32")] pub [u8; 32]);

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
pub struct TxId(#[serde(with = "hex_bytes_32")] pub [u8; 32]);

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
pub struct ActionId(#[serde(with = "hex_bytes_16")] pub [u8; 16]);

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
pub struct ProcessId(#[serde(with = "hex_bytes_16")] pub [u8; 16]);

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

/// Helper module for hex serialization of 32-byte arrays.
mod hex_bytes_32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("expected 32 bytes"))
    }
}

/// Helper module for hex serialization of 16-byte arrays.
mod hex_bytes_16 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8; 16], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 16], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("expected 16 bytes"))
    }
}

// Re-export hex for convenience
pub use hex;

#[cfg(test)]
mod tests {
    use super::*;

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
