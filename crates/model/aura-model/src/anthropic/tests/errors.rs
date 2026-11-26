//! `ApiError` classification and Cloudflare block detection.

use super::support::*;
use crate::anthropic::{AnthropicProvider, ApiError};
use crate::{ModelProvider, ModelRequestKind, ReasonerError};
use std::time::Duration;

#[test]
fn test_api_error_classification() {
    let overloaded: ReasonerError = ApiError::Overloaded {
        message: "529 overloaded".into(),
        retry_after: None,
    }
    .into();
    assert!(overloaded.to_string().contains("529"));

    let credits: ReasonerError = ApiError::InsufficientCredits("402 insufficient".into()).into();
    assert!(credits.to_string().contains("402"));

    let cloudflare: ReasonerError = ApiError::CloudflareBlock {
        message: "Cloudflare block".into(),
        wire_body_bytes: Some(42_000),
    }
    .into();
    assert!(cloudflare.to_string().contains("Cloudflare"));

    let other: ReasonerError =
        ApiError::Other(ReasonerError::Request("network error".into())).into();
    assert!(other.to_string().contains("network error"));

    // Phase 5: generic 5xx now round-trips into the dedicated
    // `ReasonerError::Transient` variant so callers can detect
    // retryable upstream blips without re-deriving classification
    // from the HTTP status. The body preview the dev loop already
    // surfaces in `task_failed` reasons is preserved.
    let transient_5xx: ReasonerError = ApiError::TransientServer {
        status: 500,
        message: "Model provider API error: 500 Internal Server Error - body".into(),
    }
    .into();
    match transient_5xx {
        ReasonerError::Transient {
            status,
            ref message,
            retry_after,
        } => {
            assert_eq!(status, 500);
            assert!(
                message.contains("Internal Server Error"),
                "TransientServer should preserve the body preview: {message}"
            );
            assert!(
                retry_after.is_none(),
                "TransientServer carries no Retry-After hint by default"
            );
        }
        other => panic!("TransientServer should map to ReasonerError::Transient, got {other:?}"),
    }
}

#[test]
fn test_overloaded_message_appends_retry_after_hint_when_absent() {
    let err: ReasonerError = ApiError::Overloaded {
        message: "Model provider API error: 429 Too Many Requests - server busy".into(),
        retry_after: Some(Duration::from_secs(7)),
    }
    .into();
    let msg = err.to_string();
    assert!(
        msg.contains("retry after 7 seconds"),
        "message should include the retry-after hint when not already present: {msg}"
    );
}

#[test]
fn test_overloaded_message_leaves_existing_retry_after_phrase_alone() {
    let err: ReasonerError = ApiError::Overloaded {
        message: "Model provider API error: 429 - \
                  {\"error\":{\"code\":\"RATE_LIMITED\",\"message\":\"Too many requests. Retry after 7 seconds.\"}}"
            .into(),
        retry_after: Some(Duration::from_secs(7)),
    }
    .into();
    let msg = err.to_string();
    let occurrences = msg.to_ascii_lowercase().matches("retry after").count();
    assert_eq!(
        occurrences, 1,
        "should not double-append the retry-after phrase: {msg}"
    );
}

#[test]
fn test_cloudflare_detection() {
    use crate::anthropic::is_cloudflare_html;
    assert!(is_cloudflare_html(
        r#"<!DOCTYPE html><!--[if lt IE 7]> <html class="no-js ie6 oldie" lang="en-US">"#
    ));
    assert!(is_cloudflare_html(
        r#"<!DOCTYPE html><html><body><p>Your request was blocked by this site's web application firewall (WAF).</p></body></html>"#
    ));
    assert!(!is_cloudflare_html(
        r#"{"error":{"type":"authentication_error","message":"invalid api key"}}"#
    ));
}

/// Runs a synchronous `#[test]` rather than `#[tokio::test]` so the
/// tracing subscriber stays installed for the entire round-trip and
/// is not torn down or shadowed across `tokio`'s task hops. Capture
/// is serialized via [`install_console_capture`].
#[test]
fn cloudflare_block_round_trip_emits_paired_request_and_failure_blocks() {
    let capture = install_console_capture();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let result = runtime.block_on(async {
        let body = r#"<!DOCTYPE html><html><body>
                <p>Your request was blocked by this site's web application firewall (WAF).</p>
                <p>Request ID: <code>cf-test-paired</code></p>
            </body></html>"#;
        let (addr, server) = serve_cloudflare_block_once(body).await;

        let provider =
            AnthropicProvider::new(proxy_config("aura-claude-sonnet-4-6", addr)).unwrap();
        let request = proxy_request("aura-claude-sonnet-4-6")
            .request_kind(ModelRequestKind::Chat)
            .try_build()
            .unwrap();

        let result = provider.complete(request).await;
        server.await.unwrap();
        result
    });

    let captured = capture.snapshot();
    assert!(result.is_err(), "Cloudflare block should surface as error");
    assert!(
        captured.contains("→ POST /v1/messages"),
        "expected paired outbound request block, got transcript:\n{captured}"
    );
    assert!(
        captured.contains("← 403 Forbidden"),
        "expected paired failure block with 403 header, got transcript:\n{captured}"
    );
    assert!(
        captured.contains("cloudflare_block"),
        "expected class label in failure block, got transcript:\n{captured}"
    );
    assert!(
        captured.contains("✗ propagate") || captured.contains("↻ retry"),
        "expected retry-decision continuation line, got transcript:\n{captured}"
    );
}

#[tokio::test]
async fn test_cloudflare_html_maps_to_typed_diagnostic_with_profile() {
    let body = r#"<!DOCTYPE html><html><body>
            <p>Your request was blocked by this site's web application firewall (WAF).</p>
            <p>Request ID: <code>cf-test-123</code></p>
        </body></html>"#;
    let (addr, server) = serve_cloudflare_block_once(body).await;

    let provider = AnthropicProvider::new(proxy_config("aura-claude-sonnet-4-6", addr)).unwrap();
    let request = proxy_request("aura-claude-sonnet-4-6")
        .request_kind(ModelRequestKind::Chat)
        .try_build()
        .unwrap();

    let err = provider.complete(request).await.unwrap_err();
    server.await.unwrap();

    match err {
        ReasonerError::Transient {
            status, message, ..
        } => {
            assert_eq!(status, 403);
            assert!(message.contains("Cloudflare block"));
            assert!(message.contains("request_id=cf-test-123"));
            assert!(message.contains("kind=Chat"));
            assert!(message.contains("content_signature="));
        }
        other => panic!("expected typed Cloudflare transient diagnostic, got {other:?}"),
    }
}
