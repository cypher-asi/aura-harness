//! # aura-auth
//!
//! Authentication client and credential storage for the Aura CLI.
//!
//! Provides:
//! - [`ZosClient`] for authenticating against the zOS API (`zosapi.zero.tech`)
//! - [`CredentialStore`] for persisting JWT tokens to `~/.aura/credentials.json`
//! - [`StoredSession`] as the serializable session type
//!
//! # Login flow
//!
//! 1. Prompt the user for email and password.
//! 2. Call [`ZosClient::login`] to obtain a JWT access token.
//! 3. Call [`CredentialStore::save`] to persist the session to disk.
//! 4. The JWT is then available via [`CredentialStore::load_token`] for proxy
//!    mode requests.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

mod credentials;
mod error;
mod redact;
mod zos_client;

pub use credentials::{CredentialStore, StoredSession};
pub use error::AuthError;
pub use redact::redact_error;
pub use zos_client::ZosClient;
