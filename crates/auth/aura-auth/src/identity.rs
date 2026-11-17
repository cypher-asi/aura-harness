//! The active identity: local (no account) or a signed-in zOS user.
//!
//! The local identity has no network or storage of its own, so it lives
//! here rather than in a provider crate. zOS sign-in is implemented by
//! `aura-auth-zos`.

use aura_core_auth::{Owner, ProviderKind, StoredSession};

/// Who owns subscription links on this machine right now.
#[derive(Debug, Clone)]
pub enum Identity {
    /// Signed out; links are owned by [`Owner::Local`].
    Local,
    /// Signed in to zOS; links are owned by the zOS user id.
    Zos(StoredSession),
}

impl Identity {
    /// Provider backing this identity.
    #[must_use]
    pub fn provider(&self) -> ProviderKind {
        match self {
            Self::Local => ProviderKind::Local,
            Self::Zos(_) => ProviderKind::Zos,
        }
    }

    /// Owner key for links created under this identity.
    #[must_use]
    pub fn owner(&self) -> Owner {
        match self {
            Self::Local => Owner::Local,
            Self::Zos(session) => session.owner(),
        }
    }

    /// Display name, `None` when local.
    #[must_use]
    pub fn display_name(&self) -> Option<&str> {
        match self {
            Self::Local => None,
            Self::Zos(session) => Some(&session.display_name),
        }
    }

    /// zOS JWT for aura-router, `None` when local.
    #[must_use]
    pub fn router_token(&self) -> Option<&str> {
        match self {
            Self::Local => None,
            Self::Zos(session) => Some(&session.access_token),
        }
    }
}
