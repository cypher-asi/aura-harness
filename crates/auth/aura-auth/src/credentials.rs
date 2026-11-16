//! Static zOS session store kept for existing callers.
//!
//! Thin wrapper over [`Vault::default_location`]; new code should use
//! [`crate::AuthManager`].

use crate::error::AuthError;
use crate::vault::Vault;
pub use aura_core_auth::StoredSession;

/// zOS session storage backed by the OS keyring with a file fallback.
pub struct CredentialStore;

impl CredentialStore {
    /// Save a zOS session.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::CredentialKeyring`] on unexpected keyring
    /// failures, [`AuthError::NoHomeDir`] when the fallback path cannot be
    /// resolved, or [`AuthError::CredentialIo`] on filesystem failures.
    pub fn save(session: &StoredSession) -> Result<(), AuthError> {
        Vault::default_location()?.save_zos_session(session)
    }

    /// Load the stored zOS session, if any.
    #[must_use]
    pub fn load() -> Option<StoredSession> {
        Vault::default_location().ok()?.zos_session()
    }

    /// Convenience: load only the JWT access token.
    #[must_use]
    pub fn load_token() -> Option<String> {
        Self::load().map(|s| s.access_token)
    }

    /// Delete the zOS session from both the keyring and the fallback
    /// file. Linked subscriptions are untouched.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::CredentialKeyring`] only on unexpected keyring
    /// failures.
    pub fn clear() -> Result<(), AuthError> {
        Vault::default_location()?.clear_zos_session()
    }
}
