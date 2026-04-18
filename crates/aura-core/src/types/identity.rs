//! Agent identity types.

use crate::ids::AgentId;
use crate::permissions::AgentPermissions;
use serde::{Deserialize, Serialize};

/// Agent identity information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Agent identifier
    pub agent_id: AgentId,
    /// ZNS identifier (e.g., "0://Agent09")
    pub zns_id: String,
    /// Mutable display name
    pub name: String,
    /// Fingerprint of the identity
    #[serde(with = "crate::serde_helpers::hex_bytes_32")]
    pub identity_hash: [u8; 32],
    /// Scope + capability bundle attached to this agent. Required on
    /// every `Identity`; there is no "legacy, unknown" fallback and no
    /// serde default. Use [`AgentPermissions::empty`] for an agent with
    /// no grants, or [`AgentPermissions::ceo_preset`] for the bootstrap
    /// super-agent (universe scope + all capabilities).
    pub permissions: AgentPermissions,
}

impl Identity {
    /// Create a new identity with empty permissions (no grants). Callers
    /// that need a non-empty grant should chain [`Self::with_permissions`].
    #[must_use]
    pub fn new(zns_id: impl Into<String>, name: impl Into<String>) -> Self {
        let zns_id = zns_id.into();
        let name = name.into();

        let identity_hash = *blake3::hash(zns_id.as_bytes()).as_bytes();
        let agent_id = AgentId::new(identity_hash);

        Self {
            agent_id,
            zns_id,
            name,
            identity_hash,
            permissions: AgentPermissions::empty(),
        }
    }

    /// Replace this identity's [`AgentPermissions`].
    #[must_use]
    pub fn with_permissions(mut self, permissions: AgentPermissions) -> Self {
        self.permissions = permissions;
        self
    }
}
