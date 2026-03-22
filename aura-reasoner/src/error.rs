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
