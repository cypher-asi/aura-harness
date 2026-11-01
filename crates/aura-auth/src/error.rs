//! Error types for the auth composition crate.

use std::path::PathBuf;

pub use aura_auth_zos::ZosError;

/// Errors from credential storage or any auth provider.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// zOS API client failure.
    #[error(transparent)]
    Zos(#[from] ZosError),

    /// A provider-level failure (sign-in, refresh, validation).
    #[error(transparent)]
    Provider(#[from] aura_core_auth::AuthError),

    /// Failed to read or write the credentials file.
    #[error("credential file I/O error at {path}: {source}")]
    CredentialIo {
        /// File that failed.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },

    /// Failed to serialize or deserialize credential data.
    #[error("credential serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Could not determine the user's home directory for credential storage.
    #[error("unable to determine home directory for credential storage")]
    NoHomeDir,

    /// Unexpected OS keyring failure while saving / loading credentials.
    ///
    /// "No storage" and "no entry" failures are handled transparently
    /// by the vault; this variant carries only the keyring errors we
    /// don't know how to recover from. (Wave 5 / T5.)
    #[error("keyring error: {0}")]
    CredentialKeyring(String),
}

impl From<AuthError> for aura_core_auth::AuthError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::Provider(e) => e,
            AuthError::Zos(e) => e.into(),
            AuthError::Serialization(e) => Self::Serialization(e),
            other => Self::Storage(other.to_string()),
        }
    }
}
