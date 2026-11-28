//! New identifier newtypes for the layered architecture.
//!
//! # Invariants
//!
//! - Fixed-size byte layouts so hex display widths are stable.
//! - `Hash`/`Display`/`Debug` mask long ids to the first 16 hex chars
//!   to keep logs readable.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! define_id_16 {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub [u8; 16]);

        impl $name {
            /// Construct from raw bytes.
            #[must_use]
            pub const fn new(bytes: [u8; 16]) -> Self { Self(bytes) }

            /// Borrow the inner byte array.
            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; 16] { &self.0 }

            /// Hex encoding.
            #[must_use]
            pub fn to_hex(&self) -> String { hex::encode(self.0) }

            /// Parse from a hex string.
            ///
            /// # Errors
            ///
            /// Returns [`hex::FromHexError`] when the input is not a
            /// 32-character hex string.
            pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
                let bytes = hex::decode(s)?;
                let arr: [u8; 16] = bytes
                    .try_into()
                    .map_err(|_| hex::FromHexError::InvalidStringLength)?;
                Ok(Self(arr))
            }

            /// Generate a random id.
            #[must_use]
            pub fn generate() -> Self { Self(*uuid::Uuid::new_v4().as_bytes()) }

            /// Derive an id from a UUID.
            #[must_use]
            pub fn from_uuid(uuid: uuid::Uuid) -> Self { Self(*uuid.as_bytes()) }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.to_hex())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.to_hex())
            }
        }
    };
}

macro_rules! define_id_32 {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub [u8; 32]);

        impl $name {
            /// Construct from raw bytes.
            #[must_use]
            pub const fn new(bytes: [u8; 32]) -> Self { Self(bytes) }

            /// Borrow the inner byte array.
            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; 32] { &self.0 }

            /// Hex encoding.
            #[must_use]
            pub fn to_hex(&self) -> String { hex::encode(self.0) }

            /// Parse from a hex string.
            ///
            /// # Errors
            ///
            /// Returns [`hex::FromHexError`] when the input is not a
            /// 64-character hex string.
            pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
                let bytes = hex::decode(s)?;
                let arr: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| hex::FromHexError::InvalidStringLength)?;
                Ok(Self(arr))
            }

            /// Derive an id by hashing a UUID's bytes with blake3.
            #[must_use]
            pub fn from_uuid(uuid: uuid::Uuid) -> Self {
                Self(*blake3::hash(uuid.as_bytes()).as_bytes())
            }

            /// Generate a random id.
            #[must_use]
            pub fn generate() -> Self { Self::from_uuid(uuid::Uuid::new_v4()) }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let hex = self.to_hex();
                let trunc = if hex.len() > 16 { &hex[..16] } else { &hex };
                write!(f, "{}({})", stringify!($name), trunc)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let hex = self.to_hex();
                if hex.len() > 16 { write!(f, "{}", &hex[..16]) } else { write!(f, "{hex}") }
            }
        }
    };
}

define_id_16! {
    /// Identifier for an agent turn — one model→tool→model round.
    TurnId
}

define_id_16! {
    /// Identifier for a logical session "run" (multiple turns).
    RunId
}

define_id_16! {
    /// Identifier minted by the model layer for every tool call.
    ToolCallId
}

define_id_16! {
    /// Opaque end-user identifier.
    UserId
}

define_id_16! {
    /// Surface session identifier (one per connected client).
    SessionId
}

define_id_32! {
    /// Content-addressed transaction id (legacy `Hash`-shaped).
    TransactionId
}

#[cfg(test)]
mod tests;

// ============================================================================
// Legacy identifiers (migrated from `aura-core`).
//
// These predate the `define_id_16!`/`define_id_32!` newtypes above and use a
// separate `define_id!` macro that wires hex serde via the `serde_helpers`
// module. They are preserved verbatim so downstream call sites keep compiling.
// ============================================================================

macro_rules! define_id {
    (
        $(#[$meta:meta])*
        $name:ident, $len:expr, $serde_mod:expr, truncate = $trunc:expr
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(#[serde(with = $serde_mod)] pub [u8; $len]);

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

/// Start of the ZNS id of an agent another agent spawned,
/// `0://spawn/<agent id hex>`: it names the id its spawner gave it.
pub const SPAWN_ZNS_PREFIX: &str = "0://spawn/";

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

    /// The id of the agent with ZNS id `zns_id`: the one a spawned
    /// agent's [`SPAWN_ZNS_PREFIX`] id names, else the blake3 hash of
    /// `zns_id`.
    #[must_use]
    pub fn of_zns(zns_id: &str) -> Self {
        zns_id
            .strip_prefix(SPAWN_ZNS_PREFIX)
            .and_then(|hex| Self::from_hex(hex).ok())
            .unwrap_or_else(|| Self(*blake3::hash(zns_id.as_bytes()).as_bytes()))
    }

    /// The ZNS id of the spawned agent with this id, which
    /// [`Self::of_zns`] maps back to it.
    #[must_use]
    pub fn spawn_zns(&self) -> String {
        format!("{SPAWN_ZNS_PREFIX}{}", self.to_hex())
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

#[cfg(test)]
mod legacy_id_tests;
