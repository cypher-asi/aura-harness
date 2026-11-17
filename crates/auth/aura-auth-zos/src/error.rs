//! zOS client errors.

use aura_core_auth::AuthError;

/// Errors from the zOS API client.
#[derive(Debug, thiserror::Error)]
pub enum ZosError {
    /// HTTP request to zOS API failed.
    #[error("zOS API request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// zOS API returned an error response.
    #[error("zOS API error (status {status}): {message}")]
    Api {
        /// HTTP status.
        status: u16,
        /// zOS error code.
        code: String,
        /// Redacted, truncated message.
        message: String,
    },
}

impl From<ZosError> for AuthError {
    fn from(error: ZosError) -> Self {
        match error {
            ZosError::Http(e) => aura_auth_kit::http::unavailable("zOS", &e),
            ZosError::Api {
                status,
                code,
                message,
            } => AuthError::Rejected {
                status,
                code,
                message,
            },
        }
    }
}
