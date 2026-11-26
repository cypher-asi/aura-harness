use crate::anthropic::provider::errors::{
    classify_transport_error, parse_retry_after_from_body, parse_retry_after_header,
    parse_retry_after_prose, provider_api_error_message,
};
use crate::anthropic::ApiError;
use crate::error::ReasonerError;
use reqwest::header::{HeaderMap, HeaderValue};
use std::time::Duration;

#[test]
fn transport_builder_errors_are_not_retryable() {
    let error = reqwest::Client::new()
        .post("://invalid-url")
        .build()
        .unwrap_err();
    assert!(error.is_builder());
    assert!(matches!(
        classify_transport_error(&error, "invalid request"),
        ApiError::Other(ReasonerError::Request(_))
    ));
}

#[test]
fn retry_after_header_parses_integer_seconds() {
    let mut headers = HeaderMap::new();
    headers.insert(reqwest::header::RETRY_AFTER, HeaderValue::from_static("7"));
    assert_eq!(
        parse_retry_after_header(&headers),
        Some(Duration::from_secs(7))
    );
}

#[test]
fn retry_after_header_absent_is_none() {
    let headers = HeaderMap::new();
    assert_eq!(parse_retry_after_header(&headers), None);
}

#[test]
fn retry_after_body_parses_aura_router_shape() {
    let body = r#"{"error":{"code":"RATE_LIMITED","message":"Too many requests. Retry after 7 seconds."}}"#;
    assert_eq!(
        parse_retry_after_from_body(body),
        Some(Duration::from_secs(7))
    );
}

#[test]
fn retry_after_body_parses_structured_retry_after() {
    let body = r#"{"error":{"code":"RATE_LIMITED","retry_after":12,"message":"slow down"}}"#;
    assert_eq!(
        parse_retry_after_from_body(body),
        Some(Duration::from_secs(12))
    );
}

#[test]
fn retry_after_body_parses_top_level_retry_after() {
    let body = r#"{"retry_after":3,"error":{"code":"OTHER"}}"#;
    assert_eq!(
        parse_retry_after_from_body(body),
        Some(Duration::from_secs(3))
    );
}

#[test]
fn retry_after_body_returns_none_when_absent() {
    let body = r#"{"error":{"code":"RATE_LIMITED","message":"slow down"}}"#;
    assert_eq!(parse_retry_after_from_body(body), None);
}

#[test]
fn provider_api_error_message_does_not_misidentify_routed_provider() {
    let body = r#"{"error":{"message":"You exceeded your current quota."}}"#;
    let message = provider_api_error_message(reqwest::StatusCode::TOO_MANY_REQUESTS, body);

    assert_eq!(
        message,
        format!("Model provider API error: 429 Too Many Requests - {body}")
    );
    assert!(!message.contains("Anthropic"));
}

#[test]
fn retry_after_prose_is_case_insensitive_and_handles_plural() {
    assert_eq!(
        parse_retry_after_prose("please Retry After 5 seconds"),
        Some(Duration::from_secs(5))
    );
    assert_eq!(
        parse_retry_after_prose("retry after 1 second"),
        Some(Duration::from_secs(1))
    );
}
