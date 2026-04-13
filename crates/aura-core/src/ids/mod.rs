//! Strongly-typed identifiers for the Aura system.
//!
//! All IDs are fixed-size byte arrays with display formatting and serialization.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! define_id {
    (
        $(#[$meta:meta])*
        $name:ident, $len:expr, $serde_mod:expr, truncate = $trunc:expr
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(#[serde(with = $serde_mod)] pub [u8; $len]);

        #[allow(deprecated)]
        impl $name {
            #[must_use]
            pub const fn new(bytes: [u8; $len]) -> Self {
                Self(bytes)
            }

            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; $len] {
                &self.0
            }

            #[must_use]
            pub fn to_hex(&self) -> String {
                hex::encode(self.0)
            }

            /// # Errors
            /// Returns error if hex string is invalid or wrong length.
            pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
                let bytes = hex::decode(s)?;
                let arr: [u8; $len] = bytes
                    .try_into()
                    .map_err(|_| hex::FromHexError::InvalidStringLength)?;
                Ok(Self(arr))
            }
        }

        #[allow(deprecated)]
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let hex = self.to_hex();
                let display = if $trunc > 0 && hex.len() > $trunc {
                    &hex[..$trunc]
                } else {
                    &hex
                };
                write!(f, "{}({})", stringify!($name), display)
            }
        }

        #[allow(deprecated)]
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let hex = self.to_hex();
                if $trunc > 0 && hex.len() > $trunc {
                    write!(f, "{}", &hex[..$trunc])
                } else {
                    write!(f, "{}", hex)
                }
            }
        }
    };
}

// ============================================================================
// Hash Type (32 bytes, blake3)
// ============================================================================

define_id!(
    /// A 32-byte blake3 hash used for transaction chaining.
    Hash, 32, "crate::serde_helpers::hex_bytes_32", truncate = 16
);

impl Hash {
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
}

// ============================================================================
// Agent ID (32 bytes)
// ============================================================================

define_id!(
    /// Agent identifier - 32 bytes, derived from identity hash or UUID.
    AgentId, 32, "crate::serde_helpers::hex_bytes_32", truncate = 16
);

impl AgentId {
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
}

// ============================================================================
// Transaction ID (32 bytes)
// ============================================================================

define_id!(
    #[deprecated(note = "use Hash — TxId is a legacy alias")]
    /// Transaction identifier - 32 bytes, typically a hash of tx content.
    TxId,
    32,
    "crate::serde_helpers::hex_bytes_32",
    truncate = 16
);

#[allow(deprecated)]
impl TxId {
    /// Generate a `TxId` by hashing content.
    #[must_use]
    pub fn from_content(content: &[u8]) -> Self {
        let hash = blake3::hash(content);
        Self(*hash.as_bytes())
    }
}

// ============================================================================
// Action ID (16 bytes)
// ============================================================================

define_id!(
    /// Action identifier - 16 bytes, generated per action.
    ActionId, 16, "crate::serde_helpers::hex_bytes_16", truncate = 0
);

impl ActionId {
    /// Generate a new random `ActionId`.
    #[must_use]
    pub fn generate() -> Self {
        let uuid = uuid::Uuid::new_v4();
        Self(*uuid.as_bytes())
    }
}

// ============================================================================
// Process ID (16 bytes)
// ============================================================================

define_id!(
    /// Process identifier - 16 bytes, generated per async process.
    ProcessId, 16, "crate::serde_helpers::hex_bytes_16", truncate = 0
);

impl ProcessId {
    /// Generate a new random `ProcessId`.
    #[must_use]
    pub fn generate() -> Self {
        let uuid = uuid::Uuid::new_v4();
        Self(*uuid.as_bytes())
    }
}

// ============================================================================
// Fact ID (16 bytes)
// ============================================================================

define_id!(
    /// Fact identifier - 16 bytes.
    FactId, 16, "crate::serde_helpers::hex_bytes_16", truncate = 0
);

impl FactId {
    /// Generate a new random `FactId`.
    #[must_use]
    pub fn generate() -> Self {
        let uuid = uuid::Uuid::new_v4();
        Self(*uuid.as_bytes())
    }
}

// ============================================================================
// Agent Event ID (16 bytes)
// ============================================================================

define_id!(
    /// Agent event identifier - 16 bytes.
    AgentEventId, 16, "crate::serde_helpers::hex_bytes_16", truncate = 0
);

impl AgentEventId {
    /// Generate a new random `AgentEventId`.
    #[must_use]
    pub fn generate() -> Self {
        let uuid = uuid::Uuid::new_v4();
        Self(*uuid.as_bytes())
    }
}

// ============================================================================
// Procedure ID (16 bytes)
// ============================================================================

define_id!(
    /// Procedure identifier - 16 bytes.
    ProcedureId, 16, "crate::serde_helpers::hex_bytes_16", truncate = 0
);

impl ProcedureId {
    /// Generate a new random `ProcedureId`.
    #[must_use]
    pub fn generate() -> Self {
        let uuid = uuid::Uuid::new_v4();
        Self(*uuid.as_bytes())
    }
}

// Re-export hex for crate-internal convenience
pub(crate) use hex;

#[cfg(test)]
mod tests;
