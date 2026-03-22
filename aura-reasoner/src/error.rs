//! Typed errors for the reasoner crate.
//!
//! [`ReasonerError`] classifies provider failures so that retry and fallback
//! logic can branch on the error *variant* rather than string-matching status
//! codes embedded in the error message.

/// Classified model-provider error.
///
/// Returned (wrapped in [`anyhow::Error`]) from [`ModelProvider::complete`](crate::ModelProvider::complete)
/// implementations. Consumers can recover the variant with
/// [`anyhow::Error::downcast_ref`].
#[derive(Debug, thiserror::Error)]
pub enum ReasonerError {
    /// 429 / 529 — the provider is rate-limiting or overloaded.
    /// Eligible for exponential backoff and model fallback.
    #[error("Rate limited: {0}")]
    RateLimited(String),

    /// 402 — insufficient credits. Must stop immediately.
    #[error("Insufficient credits: {0}")]
    InsufficientCredits(String),

    /// Any other provider-level failure (network, auth, bad request, …).
    #[error("Provider error: {0}")]
    Provider(String),
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
    fn test_provider_error_display() {
        let err = ReasonerError::Provider("network timeout".to_string());
        let msg = format!("{err}");
        assert_eq!(msg, "Provider error: network timeout");
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
        let err: anyhow::Error =
            ReasonerError::InsufficientCredits("402".to_string()).into();
        let downcasted = err.downcast_ref::<ReasonerError>();
        assert!(matches!(
            downcasted,
            Some(ReasonerError::InsufficientCredits(_))
        ));
    }

    #[test]
    fn test_debug_formatting() {
        let err = ReasonerError::Provider("bad request".to_string());
        let mut buf = String::new();
        write!(&mut buf, "{err:?}").unwrap();
        assert!(buf.contains("Provider"));
        assert!(buf.contains("bad request"));
    }
}
