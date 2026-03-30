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
mod tests;
