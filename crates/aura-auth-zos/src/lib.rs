//! # aura-auth-zos
//!
//! Layer: surface
//!
//! zOS identity provider. [`ZosClient`] authenticates against
//! `zosapi.zero.tech` with email and password; [`ZosIdentity`] adapts
//! it to [`aura_core_auth::IdentityProvider`] so subscription links can
//! be owned by the signed-in zOS user.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

mod client;
mod error;

pub use client::ZosClient;
pub use error::ZosError;

use async_trait::async_trait;
use aura_core_auth::{AuthError, IdentityProvider, ProviderKind, StoredSession};

/// [`IdentityProvider`] backed by [`ZosClient`].
pub struct ZosIdentity {
    client: ZosClient,
}

impl ZosIdentity {
    /// Build the provider with a default client.
    ///
    /// # Errors
    ///
    /// Returns [`ZosError::Http`] if the HTTP client cannot be built.
    pub fn new() -> Result<Self, ZosError> {
        Ok(Self {
            client: ZosClient::new()?,
        })
    }

    /// The underlying client.
    #[must_use]
    pub fn client(&self) -> &ZosClient {
        &self.client
    }
}

#[async_trait]
impl IdentityProvider for ZosIdentity {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Zos
    }

    async fn login(&self, email: &str, password: &str) -> Result<StoredSession, AuthError> {
        self.client
            .login(email, password)
            .await
            .map_err(AuthError::from)
    }

    async fn logout(&self, session: &StoredSession) {
        self.client.logout(&session.access_token).await;
    }
}
