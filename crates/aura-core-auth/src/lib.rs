//! # aura-core-auth
//!
//! Layer: core
//!
//! Pure auth primitive types and provider traits only — no network, no
//! storage, no provider implementations. Each auth method lives in its
//! own surface-layer crate (`aura-auth-zos`, `aura-auth-chatgpt`,
//! `aura-auth-claude`) implementing the traits below; `aura-auth`
//! composes them with credential storage.
//!
//! ## Invariants
//!
//! - [`StoredSession`] serialises exactly the same JSON as the legacy
//!   `aura_auth::StoredSession` — moving it here is wire-compatible.
//! - [`AccessToken`] / [`RefreshToken`] are opaque strings; the crate
//!   has no opinion on format (JWT vs. opaque) — that lives in the
//!   provider implementation.
//! - The model layer depends on [`CredentialResolver`] only, never on
//!   a concrete auth backend.
//!
//! ## Failure modes
//!
//! - [`AuthError`] is a closed taxonomy of primitive failure modes
//!   that any auth backend can produce without dragging in
//!   network/keyring dependencies. Backends layer richer error
//!   variants on top via `#[from]`.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

mod oauth;
mod provider;
mod traits;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use aura_core_types::UserId;
pub use oauth::{
    AuthorizationRequest, CallbackParams, InferenceCredential, LinkGrant, LinkStatus,
    LinkedAccount, LoopbackRedirect, OAuthTokens, RedirectStrategy,
};
pub use provider::{Owner, ProviderKind, UnknownProvider};
pub use traits::{CredentialResolver, IdentityProvider, SubscriptionProvider};

/// Errors produced by pure auth primitives.
///
/// Network/keyring/io variants live on top of this in backend crates
/// via `#[from]` or wrapping.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// Serialisation/deserialisation of credential data failed.
    #[error("credential serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// Backend returned a structured rejection from the upstream auth
    /// provider.
    #[error("auth provider rejected the request (status {status}): {message}")]
    Rejected {
        /// HTTP-style status code returned by the provider.
        status: u16,
        /// Provider-specific error code.
        code: String,
        /// Human-readable message.
        message: String,
    },
    /// The provider returned a response that did not match the
    /// expected shape.
    #[error("malformed auth response: {0}")]
    Malformed(String),
    /// Local storage backend reported an unrecoverable error.
    #[error("auth storage error: {0}")]
    Storage(String),
    /// No account for this provider is linked to the active identity.
    #[error("no {0} account is linked; run `aura login {id}`", id = .0.id())]
    NotLinked(ProviderKind),
    /// Stored credentials were revoked or expired; the user must sign in again.
    #[error("{0}")]
    ReauthRequired(String),
    /// The provider is temporarily unreachable; stored credentials are kept.
    #[error("{0}")]
    Unavailable(String),
    /// The user declined or abandoned the sign-in.
    #[error("{0}")]
    Declined(String),
    /// A callback, state or argument failed validation.
    #[error("{0}")]
    Invalid(String),
}

/// An opaque access token issued by the upstream auth provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessToken(pub String);

impl AccessToken {
    /// Return the inner string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A refresh token, when issued by the provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshToken(pub String);

impl RefreshToken {
    /// Return the inner string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Either kind of token, tagged by kind on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Token {
    /// An access token.
    Access {
        /// Raw token value.
        value: String,
    },
    /// A refresh token.
    Refresh {
        /// Raw token value.
        value: String,
    },
}

/// Persisted zOS authentication session.
///
/// Wire-compatible with the legacy `aura_auth::StoredSession`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    /// JWT access token for the upstream proxy.
    pub access_token: String,
    /// Upstream user id.
    pub user_id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Primary zID (e.g. `0://alice`).
    pub primary_zid: String,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
}

impl StoredSession {
    /// The identity that owns subscription links created under this session.
    #[must_use]
    pub fn owner(&self) -> Owner {
        Owner::Zos {
            user_id: self.user_id.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_session_roundtrips() {
        let session = StoredSession {
            access_token: "tok".into(),
            user_id: "u".into(),
            display_name: "n".into(),
            primary_zid: "0://x".into(),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&session).unwrap();
        let back: StoredSession = serde_json::from_str(&json).unwrap();
        assert_eq!(back.user_id, session.user_id);
        assert_eq!(
            back.owner(),
            Owner::Zos {
                user_id: "u".into()
            }
        );
    }

    #[test]
    fn token_tagged_kind_serialisation() {
        let t = Token::Access { value: "x".into() };
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(json["kind"], "access");
        assert_eq!(json["value"], "x");
    }

    #[test]
    fn not_linked_names_the_login_command() {
        let message = AuthError::NotLinked(ProviderKind::Chatgpt).to_string();
        assert!(message.contains("aura login chatgpt"), "{message}");
    }
}
