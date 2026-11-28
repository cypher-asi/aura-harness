//! Errors from the Runpod tools. They reach the model as tool failures,
//! so each one names the call that failed and what to do about it, and
//! none carries the API key.

use std::time::Duration;

/// A Runpod call that did not produce a result.
#[derive(Debug, thiserror::Error)]
pub enum RunpodError {
    #[error(
        "{call}: no Runpod API key found in {location}. Add one under Integrations > Runpod \
         (create it at https://console.runpod.io/user/settings)"
    )]
    MissingKey { call: String, location: String },

    #[error(
        "{call}: Runpod rejected the API key (HTTP {status}). Check the key under \
         Integrations > Runpod, and that it has access to this resource"
    )]
    Unauthorized { call: String, status: u16 },

    #[error("{call}: not found on Runpod{detail}")]
    NotFound { call: String, detail: String },

    #[error("{call}: Runpod's rate limit was hit{retry}; try again later")]
    RateLimited { call: String, retry: String },

    #[error("{call}: Runpod returned HTTP {status}: {detail}")]
    Api {
        call: String,
        status: u16,
        detail: String,
    },

    #[error("{call}: Runpod did not answer within {secs}s")]
    Timeout { call: String, secs: u64 },

    #[error("{call}: could not reach Runpod: {reason}")]
    Unreachable { call: String, reason: String },

    #[error("{call}: unexpected response from Runpod: {reason}")]
    BadResponse { call: String, reason: String },

    #[error("{tool}: {reason}")]
    InvalidArgs { tool: String, reason: String },

    /// A spending limit refused the call; nothing was sent.
    #[error("{tool} refused by the spending limit: {reason}. Nothing was sent to Runpod")]
    Limit { tool: String, reason: String },

    #[error("could not create the HTTP client for Runpod: {0}")]
    Client(String),
}

impl RunpodError {
    pub(crate) fn invalid(tool: &str, reason: impl Into<String>) -> Self {
        Self::InvalidArgs {
            tool: tool.to_string(),
            reason: reason.into(),
        }
    }

    pub(crate) fn limit(tool: &str, reason: impl Into<String>) -> Self {
        Self::Limit {
            tool: tool.to_string(),
            reason: reason.into(),
        }
    }

    pub(crate) fn bad_response(call: &str, reason: impl Into<String>) -> Self {
        Self::BadResponse {
            call: call.to_string(),
            reason: reason.into(),
        }
    }

    /// The error for a reqwest failure under `timeout`.
    pub(crate) fn from_reqwest(call: &str, err: &reqwest::Error, timeout: Duration) -> Self {
        if err.is_timeout() {
            return Self::Timeout {
                call: call.to_string(),
                secs: timeout.as_secs() + u64::from(timeout.subsec_nanos() > 0),
            };
        }
        // The innermost source says why (DNS, refused, TLS); request
        // errors only carry the URL, never the headers.
        let mut reason = err.to_string();
        let mut source = std::error::Error::source(err);
        while let Some(inner) = source {
            reason = inner.to_string();
            source = inner.source();
        }
        Self::Unreachable {
            call: call.to_string(),
            reason,
        }
    }
}
