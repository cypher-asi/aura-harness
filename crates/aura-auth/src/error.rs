//! Error types for the auth crate.

use std::path::PathBuf;

/// Errors that can occur during zOS authentication or credential storage.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// HTTP request to zOS API failed.
    #[error("zOS API request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// zOS API returned an error response.
    #[error("zOS API error (status {status}): {message}")]
    ZosApi {
        status: u16,
        code: String,
        message: String,
    },

    /// Failed to read or write the credentials file.
    #[error("credential file I/O error at {path}: {source}")]
    CredentialIo {
        path: PathBuf,
        source: std::io::Error,
    },

    /// Failed to serialize or deserialize credential data.
    #[error("credential serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Could not determine the user's home directory for credential storage.
    #[error("unable to determine home directory for credential storage")]
    NoHomeDir,
}
