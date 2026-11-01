//! # aura-auth
//!
//! Layer: surface
//!
//! Composition root for Aura authentication. Each auth method is its
//! own unit:
//!
//! | Method  | Where                | Role                                  |
//! |---------|----------------------|---------------------------------------|
//! | local   | [`Identity::Local`]  | identity with no account              |
//! | zOS     | `aura-auth-zos`      | identity; JWT for aura-router         |
//! | ChatGPT | `aura-auth-chatgpt`  | subscription; OpenAI Responses        |
//! | Claude  | `aura-auth-claude`   | subscription; Anthropic Messages      |
//!
//! Shared OAuth plumbing is in `aura-auth-kit`; the provider traits and
//! primitives are in `aura-core-auth`. This crate adds the [`Vault`]
//! (keyring-first storage) and [`AuthManager`], which links
//! subscriptions to the active identity and implements
//! [`aura_core_auth::CredentialResolver`] for the model layer.
//!
//! Subscription links are owned by the identity active when they were
//! created: [`aura_core_auth::Owner::Local`] while signed out, or the
//! zOS user id. Signing out of zOS hides that user's links; signing
//! back in restores them.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

mod credentials;
mod error;
mod identity;
mod manager;
mod vault;

pub use credentials::{CredentialStore, StoredSession};
pub use error::{AuthError, ZosError};
pub use identity::Identity;
pub use manager::{sign_in_notice, AuthManager, AuthSummary, LinkMode, PendingLink};
pub use vault::Vault;

pub use aura_auth_kit::{open_browser, redact_error};
pub use aura_auth_zos::{ZosClient, ZosIdentity};

// Re-export the pure primitive types from aura-core-auth so callers
// can pull token primitives without depending on the backend crate.
pub use aura_core_auth::{
    AccessToken, AuthError as PrimitiveAuthError, CredentialResolver, InferenceCredential,
    LinkStatus, LinkedAccount, Owner, ProviderKind, RefreshToken, Token,
};
