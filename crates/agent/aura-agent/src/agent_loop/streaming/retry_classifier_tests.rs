use super::*;

#[test]
fn mid_stream_internal_server_error_is_retryable() {
    let err = ReasonerError::Internal(
        "stream terminated with error (model=claude-sonnet, msg_id=msg_01): Internal server error"
            .to_string(),
    );
    assert!(stream_error_is_retryable(&err));
}

#[test]
fn anthropic_overloaded_error_frame_is_retryable() {
    let err = ReasonerError::Internal(
        "stream terminated with error: overloaded_error: service is overloaded".to_string(),
    );
    assert!(stream_error_is_retryable(&err));
}

#[test]
fn insufficient_credits_is_not_retryable() {
    let err = ReasonerError::InsufficientCredits("402".to_string());
    assert!(!stream_error_is_retryable(&err));
}

#[test]
fn context_overflow_is_not_retryable() {
    let err = ReasonerError::Api {
        status: 400,
        message: "prompt is too long".to_string(),
    };
    assert!(!stream_error_is_retryable(&err));
}

#[test]
fn rate_limited_is_not_retryable_here() {
    let err = ReasonerError::RateLimited {
        message: "429 too many requests".to_string(),
        retry_after: None,
    };
    assert!(!stream_error_is_retryable(&err));
}

#[test]
fn transient_is_retryable() {
    let err = ReasonerError::Transient {
        status: 502,
        message: "bad gateway".to_string(),
        retry_after: None,
    };
    assert!(stream_error_is_retryable(&err));
}

#[test]
fn api_5xx_is_retryable() {
    assert!(stream_error_is_retryable(&ReasonerError::Api {
        status: 500,
        message: "upstream".to_string(),
    }));
    assert!(stream_error_is_retryable(&ReasonerError::Api {
        status: 502,
        message: "bad gateway".to_string(),
    }));
    assert!(!stream_error_is_retryable(&ReasonerError::Api {
        status: 400,
        message: "bad request".to_string(),
    }));
}
