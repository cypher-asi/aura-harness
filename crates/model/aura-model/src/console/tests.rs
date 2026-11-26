use super::*;

#[test]
fn extract_host_strips_scheme_and_path() {
    assert_eq!(
        extract_host("https://aura-router.onrender.com"),
        "aura-router.onrender.com"
    );
    assert_eq!(
        extract_host("http://localhost:8080/v1/messages"),
        "localhost:8080"
    );
    assert_eq!(
        extract_host("aura-router.onrender.com"),
        "aura-router.onrender.com"
    );
}

#[test]
fn request_block_renders_without_panic() {
    anthropic_request_block(&AnthropicRequestView {
        model: "claude-opus-4-6",
        kind: "DevLoopContinuation",
        body_bytes: 24_656,
        messages_count: 7,
        tools_count: 15,
        tool_choice: "auto",
        tool_names: "Read,Write,Edit,Glob,Grep,Shell,SemanticSearch",
        thinking_label: "on(medium · enabled · b=4096)",
        system_bytes: 3823,
        last_user_bytes: 932,
        last_user_hash: Some("8c65c4f3"),
        headers_present: "ver auth ct beta proj agent sess org",
        request_hash: "1e62bdae",
        destination: "aura-network",
        destination_host: "aura-router.onrender.com",
    });
}

#[test]
fn request_block_skips_tool_names_when_empty() {
    // Auxiliary-style request (no tools attached): we still want
    // the block to render, but the `tool_names` row should not
    // appear since there's nothing to enumerate. Asserting via
    // direct construction here — the renderer logs to a tracing
    // target so this is a smoke test that no panic / format
    // misalignment fires when the field is empty.
    anthropic_request_block(&AnthropicRequestView {
        model: "claude-opus-4-6",
        kind: "Auxiliary",
        body_bytes: 12_345,
        messages_count: 1,
        tools_count: 0,
        tool_choice: "n/a",
        tool_names: "",
        thinking_label: "off",
        system_bytes: 162,
        last_user_bytes: 14_700,
        last_user_hash: Some("511e97e8"),
        headers_present: "ver auth ct beta proj agent sess org",
        request_hash: "d5d47e97",
        destination: "aura-network",
        destination_host: "aura-router.onrender.com",
    });
}

#[test]
fn response_block_renders_without_panic() {
    anthropic_response_block(&AnthropicResponseView {
        status_code: 200,
        status_text: "OK",
        stop_reason: "ToolUse",
        elapsed_ms: 4_200,
        input_tokens: 12_345,
        output_tokens: 234,
        request_id: Some("req_01abcd"),
        destination: "aura-network",
    });
}

#[test]
fn failure_block_renders_403_cloudflare() {
    anthropic_failure_block(&AnthropicFailureView {
        status_code: Some(403),
        status_text: "Forbidden",
        class: "cloudflare_block",
        elapsed_ms: 612,
        request_id: Some("cf-ray-deadbeef"),
        retry_after_s: None,
        body_preview: Some(
            "<!DOCTYPE html>\n<html><body>\nyour request was blocked\n</body></html>",
        ),
        destination: "aura-network",
    });
}

#[test]
fn failure_block_renders_429_with_retry_after() {
    anthropic_failure_block(&AnthropicFailureView {
        status_code: Some(429),
        status_text: "Too Many Requests",
        class: "rate_limited_429",
        elapsed_ms: 84,
        request_id: Some("req_xxx"),
        retry_after_s: Some(7),
        body_preview: Some("{\"error\":{\"code\":\"RATE_LIMITED\"}}"),
        destination: "aura-network",
    });
}

#[test]
fn failure_block_renders_transport() {
    anthropic_failure_block(&AnthropicFailureView {
        status_code: None,
        status_text: "transport failed",
        class: "transport",
        elapsed_ms: 30_000,
        request_id: None,
        retry_after_s: None,
        body_preview: Some("connection reset by peer"),
        destination: "aura-network",
    });
}

#[test]
fn failure_block_renders_parse() {
    anthropic_failure_block(&AnthropicFailureView {
        status_code: Some(200),
        status_text: "OK",
        class: "parse",
        elapsed_ms: 1_840,
        request_id: Some("req_parse"),
        retry_after_s: None,
        body_preview: Some("expected `,` or `}` at line 1 column 87"),
        destination: "aura-network",
    });
}

#[test]
fn retry_decision_line_renders_each_variant() {
    anthropic_retry_decision_line(&RetryDecisionView::Retry {
        attempt_that_failed: 0,
        max_retries: 2,
        sleep_ms: 1500,
        body_cap_bytes: Some(192 * 1024),
    });
    anthropic_retry_decision_line(&RetryDecisionView::Retry {
        attempt_that_failed: 1,
        max_retries: 2,
        sleep_ms: 3000,
        body_cap_bytes: None,
    });
    anthropic_retry_decision_line(&RetryDecisionView::Fallback {
        next_model: "claude-haiku-4-6",
    });
    anthropic_retry_decision_line(&RetryDecisionView::Propagate {
        reason: "retries exhausted",
    });
}
