use crate::agent_loop::iteration::{looks_like_rate_limited, LlmCallError};

#[test]
fn from_reasoner_error_maps_rate_limited_variant() {
    let err = aura_reasoner::ReasonerError::RateLimited(
        "429 too many requests (retry after 7 seconds)".to_string(),
    );
    match LlmCallError::from_reasoner_error(&err) {
        LlmCallError::RateLimited(msg) => {
            assert!(msg.contains("retry after 7 seconds"), "message: {msg}");
        }
        _ => panic!("expected RateLimited"),
    }
}

#[test]
fn from_reasoner_error_recovers_rate_limited_across_kernel_boundary() {
    // Matches what `KernelModelGateway::complete_streaming` produces
    // when the kernel stringifies a rate-limit error:
    //     ReasonerError::Internal("kernel reason_streaming error: reasoner error: Rate limited: ...")
    let err = aura_reasoner::ReasonerError::Internal(
        "kernel reason_streaming error: reasoner error: Rate limited: \
         Anthropic API error: 429 Too Many Requests - \
         {\"error\":{\"code\":\"RATE_LIMITED\",\"message\":\"Too many requests. Retry after 7 seconds.\"}}"
            .to_string(),
    );
    assert!(
        matches!(
            LlmCallError::from_reasoner_error(&err),
            LlmCallError::RateLimited(_)
        ),
        "expected prose-based rate-limit recovery to kick in"
    );
}

#[test]
fn from_reasoner_error_does_not_confuse_other_errors_with_rate_limited() {
    let err = aura_reasoner::ReasonerError::Api {
        status: 500,
        message: "internal server error".to_string(),
    };
    assert!(matches!(
        LlmCallError::from_reasoner_error(&err),
        LlmCallError::Fatal(_)
    ));
}

#[test]
fn looks_like_rate_limited_is_case_insensitive() {
    assert!(looks_like_rate_limited("Rate Limited: boom"));
    assert!(looks_like_rate_limited("Too Many Requests"));
    assert!(looks_like_rate_limited("code: RATE_LIMITED"));
    assert!(!looks_like_rate_limited("invalid api key"));
}
