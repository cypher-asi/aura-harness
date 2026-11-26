//! Proxy-mode request headers and body fields, per upstream provider
//! family, and usage aliases in proxy responses.

use super::support::*;
use crate::anthropic::AnthropicProvider;
use crate::{ModelProvider, PromptCacheRetention};

#[tokio::test]
async fn test_proxy_mode_sends_caching_beta_header() {
    let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"test","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
    let (addr, server) = serve_json_once(body).await;

    let provider = AnthropicProvider::new(proxy_config("aura-claude-sonnet-4-6", addr)).unwrap();
    let request = proxy_request("aura-claude-sonnet-4-6").try_build().unwrap();

    let _ = provider.complete(request).await;

    let captured = server.await.unwrap();
    assert!(
        captured.contains("anthropic-beta"),
        "Proxy request should include anthropic-beta header.\nCaptured headers:\n{captured}"
    );
    assert!(
        captured.contains("prompt-caching-2024-07-31"),
        "anthropic-beta header should include prompt-caching beta tag.\nCaptured headers:\n{captured}"
    );
}

#[tokio::test]
async fn test_proxy_mode_forwards_aura_routing_headers() {
    let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"test","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
    let (addr, server) = serve_json_once(body).await;

    let provider = AnthropicProvider::new(proxy_config("aura-claude-sonnet-4-6", addr)).unwrap();
    let request = proxy_request("aura-claude-sonnet-4-6")
        .aura_org_id(Some("org-123".to_string()))
        .aura_session_id(Some("session-456".to_string()))
        .try_build()
        .unwrap();

    let _ = provider.complete(request).await;

    let captured = server.await.unwrap();
    let captured_lower = captured.to_ascii_lowercase();
    assert!(
        captured_lower.contains("x-aura-org-id: org-123"),
        "Proxy request should include org routing header.\nCaptured request:\n{captured}"
    );
    assert!(
        captured_lower.contains("x-aura-session-id: session-456"),
        "Proxy request should include session routing header.\nCaptured request:\n{captured}"
    );
}

#[tokio::test]
async fn test_proxy_openai_models_omit_prompt_caching_headers_and_fields() {
    let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"aura-gpt-4.1","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
    let (addr, server) = serve_json_once(body).await;

    let provider = AnthropicProvider::new(proxy_config("aura-gpt-4.1", addr)).unwrap();
    let request = proxy_request("aura-gpt-4.1").try_build().unwrap();

    let _ = provider.complete(request).await.unwrap();

    let captured = server.await.unwrap();
    assert!(
        !captured.contains("anthropic-beta"),
        "Proxy OpenAI requests should omit anthropic-beta prompt caching headers.\nCaptured request:\n{captured}"
    );
    assert!(
        !captured.contains("cache_control"),
        "Proxy OpenAI requests should omit Anthropic cache_control fields.\nCaptured request:\n{captured}"
    );
}

#[tokio::test]
async fn test_proxy_deepseek_family_uses_provider_hint_and_usage_aliases() {
    let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"deepseek-v4-flash","stop_reason":"end_turn","usage":{"prompt_tokens":100,"completion_tokens":25,"prompt_cache_miss_tokens":80,"prompt_cache_hit_tokens":20}}"#;
    let (addr, server) = serve_json_once(body).await;

    let provider = AnthropicProvider::new(proxy_config("deepseek-v4-flash", addr)).unwrap();
    let request = proxy_request("deepseek-v4-flash")
        .upstream_provider_family(Some("deepseek".to_string()))
        .try_build()
        .unwrap();

    let response = provider.complete(request).await.unwrap();

    let captured = server.await.unwrap();
    assert!(
        captured
            .to_ascii_lowercase()
            .contains("x-aura-upstream-provider-family: deepseek"),
        "Proxy DeepSeek requests should carry the upstream provider family hint.\nCaptured request:\n{captured}"
    );
    assert!(
        !captured.contains("anthropic-beta"),
        "Proxy DeepSeek requests should omit Anthropic beta headers.\nCaptured request:\n{captured}"
    );
    assert!(
        !captured.contains("cache_control"),
        "Proxy DeepSeek requests should omit Anthropic cache_control fields.\nCaptured request:\n{captured}"
    );
    assert_eq!(response.usage.input_tokens, 100);
    assert_eq!(response.usage.output_tokens, 25);
    assert_eq!(response.usage.cache_creation_input_tokens, Some(80));
    assert_eq!(response.usage.cache_read_input_tokens, Some(20));
}

#[tokio::test]
async fn test_proxy_hint_prefers_anthropic_family_over_model_heuristics() {
    let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"aura-gpt-4.1","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
    let (addr, server) = serve_json_once(body).await;

    let provider = AnthropicProvider::new(proxy_config("aura-gpt-4.1", addr)).unwrap();
    let request = proxy_request("aura-gpt-4.1")
        .upstream_provider_family(Some("anthropic".to_string()))
        .try_build()
        .unwrap();

    let _ = provider.complete(request).await.unwrap();

    let captured = server.await.unwrap();
    assert!(
        captured.contains("anthropic-beta"),
        "Explicit Anthropic family hints should enable Anthropic proxy headers even for non-Claude model names.\nCaptured request:\n{captured}"
    );
    assert!(
        captured.contains("cache_control"),
        "Explicit Anthropic family hints should enable Anthropic cache_control fields even for non-Claude model names.\nCaptured request:\n{captured}"
    );
}

#[tokio::test]
async fn test_proxy_non_anthropic_family_omits_thinking_and_output_config_for_complete() {
    let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"claude-opus-4-6","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
    let (addr, server) = serve_json_once(body).await;

    let provider = AnthropicProvider::new(proxy_config("claude-opus-4-6", addr)).unwrap();
    let request = proxy_request("claude-opus-4-6")
        .max_tokens(8192)
        .upstream_provider_family(Some("openai".to_string()))
        .try_build()
        .unwrap();

    let _ = provider.complete(request).await.unwrap();

    let captured = server.await.unwrap();
    assert!(
        !captured.contains(r#""thinking":"#),
        "Explicit non-Anthropic family hints should suppress Anthropic thinking config for complete requests.\nCaptured request:\n{captured}"
    );
    assert!(
        !captured.contains("output_config"),
        "Explicit non-Anthropic family hints should suppress Anthropic output config for complete requests.\nCaptured request:\n{captured}"
    );
}

#[tokio::test]
async fn test_proxy_openai_models_carry_prompt_cache_key_when_set() {
    let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"aura-gpt-4.1","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
    let (addr, server) = serve_json_once(body).await;

    let provider = AnthropicProvider::new(proxy_config("aura-gpt-4.1", addr)).unwrap();
    let request = proxy_request("aura-gpt-4.1")
        .prompt_cache_key(Some("agent:abc-123".into()))
        .prompt_cache_retention(Some(PromptCacheRetention::Hours24))
        .try_build()
        .unwrap();

    let _ = provider.complete(request).await.unwrap();

    let captured = server.await.unwrap();
    assert!(
        captured.contains(r#""prompt_cache_key":"agent:abc-123""#),
        "Proxy OpenAI requests should carry prompt_cache_key in body.\nCaptured request:\n{captured}"
    );
    assert!(
        captured.contains(r#""prompt_cache_retention":"24h""#),
        "Proxy OpenAI requests should carry prompt_cache_retention in body.\nCaptured request:\n{captured}"
    );
    assert!(
        captured
            .to_ascii_lowercase()
            .contains("x-aura-prompt-cache-key: agent:abc-123"),
        "Proxy OpenAI requests should carry X-Aura-Prompt-Cache-Key header.\nCaptured request:\n{captured}"
    );
    assert!(
        captured
            .to_ascii_lowercase()
            .contains("x-aura-prompt-cache-retention: 24h"),
        "Proxy OpenAI requests should carry X-Aura-Prompt-Cache-Retention header.\nCaptured request:\n{captured}"
    );
    assert!(
        !captured.contains("anthropic-beta"),
        "Proxy OpenAI requests should still omit anthropic-beta even with cache key set.\nCaptured request:\n{captured}"
    );
    assert!(
        !captured.contains("cache_control"),
        "Proxy OpenAI requests should still omit cache_control even with cache key set.\nCaptured request:\n{captured}"
    );
}

/// The `prompt_cache_key` value in the captured request body.
fn body_prompt_cache_key(captured: &str) -> &str {
    captured
        .split(r#""prompt_cache_key":""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("body should carry a clamped prompt_cache_key")
}

/// The `X-Aura-Prompt-Cache-Key` header value in the captured request.
fn header_prompt_cache_key(captured: &str) -> String {
    captured
        .to_ascii_lowercase()
        .split("x-aura-prompt-cache-key: ")
        .nth(1)
        .and_then(|rest| rest.split("\r\n").next())
        .map(str::to_string)
        .expect("header should carry a clamped prompt_cache_key")
}

#[tokio::test]
async fn test_proxy_openai_models_clamp_oversized_prompt_cache_key() {
    let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"aura-gpt-4.1","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
    let (addr, server) = serve_once(json_ok(body), 16384).await;

    // A composite identity well over OpenAI's 64-char limit, like the
    // `agent:{template}::{instance}` / `tool:{project}:{name}` keys the
    // upstream layers can produce.
    let oversized = format!("agent:{}", "z".repeat(200));

    let provider = AnthropicProvider::new(proxy_config("aura-gpt-4.1", addr)).unwrap();
    let request = proxy_request("aura-gpt-4.1")
        .prompt_cache_key(Some(oversized.clone()))
        .try_build()
        .unwrap();

    let _ = provider.complete(request).await.unwrap();

    let captured = server.await.unwrap();
    assert!(
        !captured.contains(&oversized),
        "Oversized prompt_cache_key must never reach the wire verbatim.\nCaptured request:\n{captured}"
    );

    // Body field stays within the 64-char limit and keeps its namespace.
    let body_key = body_prompt_cache_key(&captured);
    assert!(
        body_key.len() <= aura_core_protocol::MAX_PROMPT_CACHE_KEY_LEN,
        "Clamped body key must fit OpenAI's limit, got {}: {body_key}",
        body_key.len()
    );
    assert!(body_key.starts_with("agent:"));

    // Header value mirrors the clamped body value.
    let header_key = header_prompt_cache_key(&captured);
    assert!(
        header_key.len() <= aura_core_protocol::MAX_PROMPT_CACHE_KEY_LEN,
        "Clamped header key must fit OpenAI's limit, got {}: {header_key}",
        header_key.len()
    );
}

#[tokio::test]
async fn test_proxy_openai_response_cached_tokens_alias_maps_to_cache_read() {
    // Anthropic-shape usage block emitted by the router after it
    // flattens OpenAI's `prompt_tokens_details.cached_tokens`.
    let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"aura-gpt-4.1","stop_reason":"end_turn","usage":{"input_tokens":100,"output_tokens":25,"cached_tokens":60}}"#;
    let (addr, server) = serve_json_once(body).await;

    let provider = AnthropicProvider::new(proxy_config("aura-gpt-4.1", addr)).unwrap();
    let request = proxy_request("aura-gpt-4.1").try_build().unwrap();

    let response = provider.complete(request).await.unwrap();
    server.await.unwrap();

    assert_eq!(response.usage.input_tokens, 100);
    assert_eq!(response.usage.output_tokens, 25);
    assert_eq!(response.usage.cache_read_input_tokens, Some(60));
}
