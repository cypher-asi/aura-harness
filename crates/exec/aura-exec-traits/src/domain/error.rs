//! [`DomainError`]: how a [`DomainApi`](super::DomainApi) call fails.

/// Errors from [`DomainApi`](super::DomainApi) calls.
///
/// The domain tools show an error to the model as its `Display` text
/// alone, so every message names what failed by itself; the underlying
/// error stays reachable through `source()`. Transport errors are boxed
/// so this seam crate does not depend on the HTTP client an
/// implementation uses.
#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    /// The operation needs a user JWT and the caller passed none.
    #[error(
        "JWT required for this operation but not provided — ensure the front-end sends a token in RuntimeRequest.auth_jwt"
    )]
    MissingJwt,

    /// The request to `url` got no response. `reason` is a safe
    /// category ("request timed out", "connection failed", ...) for the
    /// message, which would otherwise read only `GET <url>`.
    #[error("{method} {url}: {reason}")]
    Transport {
        method: String,
        url: String,
        reason: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The response body could not be read.
    #[error(transparent)]
    Body(Box<dyn std::error::Error + Send + Sync>),

    /// The backend answered with the non-success HTTP `status`. `reason`
    /// is the status's canonical reason phrase, and `body` the start of
    /// the response body.
    #[error("HTTP {status} {}: {body}", status_reason(*reason))]
    Status {
        status: u16,
        reason: Option<&'static str>,
        body: String,
    },

    /// Cloudflare still blocked the request to `url` (with HTTP `status`)
    /// after the retries.
    #[error(
        "HTTP {status} {}: Cloudflare is blocking requests to {url} — the service may be cold-starting or temporarily unavailable",
        status_reason(*reason)
    )]
    CloudflareBlocked {
        status: u16,
        reason: Option<&'static str>,
        url: String,
    },

    /// The response from `url` is not the JSON the call returns.
    #[error("parse response from {url}")]
    Decode {
        url: String,
        #[source]
        source: serde_json::Error,
    },

    /// `operation` is a default [`DomainApi`](super::DomainApi) method the
    /// implementation does not override.
    #[error("{operation} is not supported by this DomainApi implementation")]
    Unsupported { operation: &'static str },

    /// The implementation does not implement `operation` (named with its
    /// type, e.g. `HttpDomainApi::create_session`).
    #[error("{operation} not implemented")]
    NotImplemented { operation: &'static str },
}

/// Result of a [`DomainApi`](super::DomainApi) call.
pub type DomainResult<T> = Result<T, DomainError>;

/// An HTTP status's reason phrase as `http::StatusCode` displays it.
fn status_reason(reason: Option<&'static str>) -> &'static str {
    reason.unwrap_or("<unknown status code>")
}

#[cfg(test)]
mod tests {
    use super::DomainError;

    #[test]
    fn messages_name_what_failed() {
        let unsupported = DomainError::Unsupported {
            operation: "append_to_spec",
        };
        assert_eq!(
            unsupported.to_string(),
            "append_to_spec is not supported by this DomainApi implementation"
        );
        let status = DomainError::Status {
            status: 404,
            reason: Some("Not Found"),
            body: "no task".to_string(),
        };
        assert_eq!(status.to_string(), "HTTP 404 Not Found: no task");
        let unknown = DomainError::Status {
            status: 599,
            reason: None,
            body: String::new(),
        };
        assert_eq!(unknown.to_string(), "HTTP 599 <unknown status code>: ");
    }
}
