//! Typed errors for the reasoner crate.
//!
//! [`ReasonerError`] classifies provider failures so that retry and fallback
//! logic can branch on the error *variant* rather than string-matching status
//! codes embedded in the error message.

use crate::types::PartialToolUse;

/// Classified model-provider error.
///
/// Returned from [`ModelProvider::complete`](crate::ModelProvider::complete) and
/// other provider implementations. Consumers can match on the variant directly.
#[derive(Debug, thiserror::Error)]
pub enum ReasonerError {
    /// 429 / 529 — the provider is rate-limiting or overloaded.
    /// Eligible for exponential backoff and model fallback.
    #[error("Rate limited: {0}")]
    RateLimited(String),

    /// 402 — insufficient credits. Must stop immediately.
    #[error("Insufficient credits: {0}")]
    InsufficientCredits(String),

    /// HTTP-level API error with status code.
    #[error("API error (status {status}): {message}")]
    Api { status: u16, message: String },

    /// Network or connection-level failure.
    #[error("request error: {0}")]
    Request(String),

    /// Request timed out.
    #[error("timeout")]
    Timeout,

    /// Failed to parse a response body.
    #[error("parse error: {0}")]
    Parse(String),

    /// A streaming response was interrupted mid-flight by a transport
    /// or SSE-level error while a `tool_use` block was still being
    /// accumulated.
    ///
    /// Carries enough context for the agent-loop to drive a
    /// per-tool-call retry (re-issuing a fresh streaming request) rather
    /// than silently fall back to a non-streaming call that would have
    /// no memory of the interrupted tool call. Returned from
    /// [`crate::types::StreamAccumulator::into_response`] when
    /// `stream_error` is set; the caller is responsible for deciding
    /// whether to retry or propagate.
    #[error("{reason}")]
    StreamAbortedWithPartial {
        /// Human-readable reason, already annotated with
        /// `model=... msg_id=... request_id=...` context when
        /// available.
        reason: String,
        /// In-flight tool-use captured just before the stream died.
        /// `None` when the error arrived before `content_block_start`.
        partial_tool_use: Option<PartialToolUse>,
    },

    /// Catch-all for other provider-level failures.
    #[error("{0}")]
    Internal(String),
}

impl ReasonerError {
    #[must_use]
    pub const fn is_insufficient_credits(&self) -> bool {
        matches!(self, Self::InsufficientCredits(_))
    }

    #[must_use]
    pub fn is_context_overflow(&self) -> bool {
        match self {
            Self::Api { status, message } => {
                *status == 413
                    || ((*status == 400 || *status == 422)
                        && message_indicates_context_overflow(message))
            }
            Self::Request(message) | Self::Parse(message) | Self::Internal(message) => {
                message_indicates_context_overflow(message)
            }
            Self::RateLimited(_)
            | Self::InsufficientCredits(_)
            | Self::Timeout
            | Self::StreamAbortedWithPartial { .. } => false,
        }
    }
}

fn message_indicates_context_overflow(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    [
        "prompt is too long",
        "prompt too long",
        "prompt too large",
        "context length exceeded",
        "context window exceeded",
        "context window limit",
        "exceeds context window",
        "exceed the model context window",
        "input length and max_tokens exceed context limit",
        "requested tokens exceed the context window",
        "request exceeds the context window",
        "too many tokens",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn test_rate_limited_display() {
        let err = ReasonerError::RateLimited("429 too many requests".to_string());
        let msg = format!("{err}");
        assert_eq!(msg, "Rate limited: 429 too many requests");
    }

    #[test]
    fn test_insufficient_credits_display() {
        let err = ReasonerError::InsufficientCredits("402 payment required".to_string());
        let msg = format!("{err}");
        assert_eq!(msg, "Insufficient credits: 402 payment required");
    }

    #[test]
    fn test_api_error_display() {
        let err = ReasonerError::Api {
            status: 500,
            message: "internal error".to_string(),
        };
        let msg = format!("{err}");
        assert_eq!(msg, "API error (status 500): internal error");
    }

    #[test]
    fn test_request_error_display() {
        let err = ReasonerError::Request("connection refused".to_string());
        assert_eq!(format!("{err}"), "request error: connection refused");
    }

    #[test]
    fn test_timeout_display() {
        let err = ReasonerError::Timeout;
        assert_eq!(format!("{err}"), "timeout");
    }

    #[test]
    fn test_parse_error_display() {
        let err = ReasonerError::Parse("invalid JSON".to_string());
        assert_eq!(format!("{err}"), "parse error: invalid JSON");
    }

    #[test]
    fn test_internal_error_display() {
        let err = ReasonerError::Internal("something broke".to_string());
        assert_eq!(format!("{err}"), "something broke");
    }

    #[test]
    fn test_downcast_from_anyhow() {
        let err: anyhow::Error = ReasonerError::RateLimited("429".to_string()).into();
        let downcasted = err.downcast_ref::<ReasonerError>();
        assert!(downcasted.is_some());
        assert!(matches!(downcasted.unwrap(), ReasonerError::RateLimited(_)));
    }

    #[test]
    fn test_downcast_insufficient_credits() {
        let err: anyhow::Error = ReasonerError::InsufficientCredits("402".to_string()).into();
        let downcasted = err.downcast_ref::<ReasonerError>();
        assert!(matches!(
            downcasted,
            Some(ReasonerError::InsufficientCredits(_))
        ));
    }

    #[test]
    fn test_debug_formatting() {
        let err = ReasonerError::Internal("bad request".to_string());
        let mut buf = String::new();
        write!(&mut buf, "{err:?}").unwrap();
        assert!(buf.contains("Internal"));
        assert!(buf.contains("bad request"));
    }

    #[test]
    fn test_context_overflow_detection_for_api_error() {
        let err = ReasonerError::Api {
            status: 400,
            message: "input length and max_tokens exceed context limit".to_string(),
        };
        assert!(err.is_context_overflow());
    }

    #[test]
    fn test_context_overflow_detection_for_413() {
        let err = ReasonerError::Api {
            status: 413,
            message: "request entity too large".to_string(),
        };
        assert!(err.is_context_overflow());
    }

    #[test]
    fn test_context_overflow_detection_ignores_other_api_errors() {
        let err = ReasonerError::Api {
            status: 400,
            message: "invalid api key".to_string(),
        };
        assert!(!err.is_context_overflow());
    }
}
