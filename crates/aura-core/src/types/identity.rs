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
    /// Phase 5: optional scope + capability bundle attached to this agent.
    ///
    /// `None` means "legacy record — no explicit grants". Phase 6's migrator
    /// backfills existing super-agents with `AgentPermissions::legacy_default`
    /// so today's behavior is preserved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<AgentPermissions>,
}

impl Identity {
    /// Create a new identity.
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
            permissions: None,
        }
    }

    /// Attach explicit [`AgentPermissions`] to this identity.
    #[must_use]
    pub fn with_permissions(mut self, permissions: AgentPermissions) -> Self {
        self.permissions = Some(permissions);
        self
    }
}
