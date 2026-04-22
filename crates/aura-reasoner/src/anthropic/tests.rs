use super::api_types::{ApiContent, ApiToolChoice};
use super::convert::{
    build_system_block, convert_messages_to_api, convert_tool_choice, convert_tools_to_api,
    resolve_output_config, resolve_thinking,
};
use super::{AnthropicConfig, AnthropicProvider, ApiError, RoutingMode};
use crate::{
    Message, ModelProvider, ModelRequest, ReasonerError, StopReason, StreamEvent, ThinkingConfig,
    ToolChoice, ToolDefinition,
};
use futures_util::StreamExt;
use std::time::Duration;

#[test]
fn test_config_new() {
    let config = AnthropicConfig::new("test-key", "claude-3-haiku");
    assert_eq!(config.api_key, "test-key");
    assert_eq!(config.default_model, "claude-3-haiku");
    assert_eq!(config.routing_mode, RoutingMode::Direct);
}

#[test]
fn test_convert_messages() {
    let messages = vec![Message::user("Hello"), Message::assistant("Hi there!")];

    let api_msgs = convert_messages_to_api(&messages, true);
    assert_eq!(api_msgs.len(), 2);
    assert_eq!(api_msgs[0].role, "user");
    assert_eq!(api_msgs[1].role, "assistant");
}

#[test]
fn test_convert_tools() {
    let tools = vec![ToolDefinition::new(
        "fs.read",
        "Read a file",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            }
        }),
    )];

    let api_tools = convert_tools_to_api(&tools, true);
    assert_eq!(api_tools.len(), 1);
    assert_eq!(api_tools[0].name, "fs.read");
}

#[test]
fn test_convert_tool_choice() {
    assert!(matches!(
        convert_tool_choice(&ToolChoice::Auto),
        Some(ApiToolChoice::Auto)
    ));
    assert!(matches!(
        convert_tool_choice(&ToolChoice::Required),
        Some(ApiToolChoice::Any)
    ));
    assert!(convert_tool_choice(&ToolChoice::None).is_none());
}

#[test]
fn test_cache_control_on_system_block() {
    let system = build_system_block("You are a helpful assistant.", true);
    let arr = system.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let block = &arr[0];
    assert_eq!(block["type"], "text");
    assert_eq!(block["text"], "You are a helpful assistant.");
    assert_eq!(block["cache_control"]["type"], "ephemeral");
}

#[test]
fn test_cache_control_on_last_tool() {
    let tools = vec![
        ToolDefinition::new(
            "fs.read",
            "Read a file",
            serde_json::json!({"type": "object"}),
        ),
        ToolDefinition::new(
            "fs.write",
            "Write a file",
            serde_json::json!({"type": "object"}),
        ),
    ];

    let api_tools = convert_tools_to_api(&tools, true);
    assert_eq!(api_tools.len(), 2);
    assert!(api_tools[0].cache_control.is_none());
    let last_cc = api_tools[1].cache_control.as_ref().unwrap();
    assert_eq!(last_cc["type"], "ephemeral");
}

#[test]
fn test_cache_control_on_last_user_message() {
    let messages = vec![
        Message::user("Hello"),
        Message::assistant("Hi!"),
        Message::user("How are you?"),
    ];

    let api_msgs = convert_messages_to_api(&messages, true);

    let last_user = &api_msgs[2];
    assert_eq!(last_user.role, "user");
    if let ApiContent::Text { cache_control, .. } = &last_user.content[0] {
        let cc = cache_control.as_ref().unwrap();
        assert_eq!(cc["type"], "ephemeral");
    } else {
        panic!("Expected Text content");
    }

    if let ApiContent::Text { cache_control, .. } = &api_msgs[0].content[0] {
        assert!(cache_control.is_none());
    }
}

#[test]
fn test_beta_header_present() {
    let config = AnthropicConfig::new("test-key", "test-model");
    let provider = AnthropicProvider::new(config).unwrap();

    let system = build_system_block("test", true);
    let json = serde_json::to_string(&system).unwrap();
    assert!(json.contains("cache_control"));
    assert!(json.contains("ephemeral"));

    assert_eq!(provider.name(), "anthropic");
}

#[test]
fn test_cache_control_omitted_when_prompt_caching_disabled() {
    let system = build_system_block("test", false);
    let json = serde_json::to_string(&system).unwrap();
    assert!(!json.contains("cache_control"));

    let messages = vec![
        Message::user("Hello"),
        Message::assistant("Hi!"),
        Message::user("How are you?"),
    ];
    let api_msgs = convert_messages_to_api(&messages, false);
    if let ApiContent::Text { cache_control, .. } = &api_msgs[2].content[0] {
        assert!(cache_control.is_none());
    } else {
        panic!("Expected Text content");
    }

    let tools = vec![
        ToolDefinition::new(
            "fs.read",
            "Read a file",
            serde_json::json!({"type": "object"}),
        ),
        ToolDefinition::new(
            "fs.write",
            "Write a file",
            serde_json::json!({"type": "object"}),
        ),
    ];
    let api_tools = convert_tools_to_api(&tools, false);
    assert!(api_tools.iter().all(|tool| tool.cache_control.is_none()));
}

const TEST_DEFAULT_MODEL: &str = "claude-opus-4-6";
const TEST_FALLBACK_MODEL: &str = "claude-sonnet-4-6";

#[test]
fn test_config_with_fallback() {
    let mut config = AnthropicConfig::new("key", TEST_DEFAULT_MODEL);
    config.fallback_model = Some(TEST_FALLBACK_MODEL.to_string());
    assert_eq!(config.fallback_model, Some(TEST_FALLBACK_MODEL.to_string()));
}

#[test]
fn test_model_chain_without_fallback() {
    let config = AnthropicConfig::new("key", TEST_DEFAULT_MODEL);
    let provider = AnthropicProvider::new(config).unwrap();
    let chain = provider.model_chain(TEST_DEFAULT_MODEL);
    assert_eq!(chain, vec![TEST_DEFAULT_MODEL]);
}

#[test]
fn test_model_chain_with_fallback() {
    let mut config = AnthropicConfig::new("key", TEST_DEFAULT_MODEL);
    config.fallback_model = Some(TEST_FALLBACK_MODEL.to_string());
    let provider = AnthropicProvider::new(config).unwrap();
    let chain = provider.model_chain(TEST_DEFAULT_MODEL);
    assert_eq!(chain, vec![TEST_DEFAULT_MODEL, TEST_FALLBACK_MODEL]);
}

#[test]
fn test_model_chain_deduplicates() {
    let mut config = AnthropicConfig::new("key", TEST_DEFAULT_MODEL);
    config.fallback_model = Some(TEST_DEFAULT_MODEL.to_string());
    let provider = AnthropicProvider::new(config).unwrap();
    let chain = provider.model_chain(TEST_DEFAULT_MODEL);
    assert_eq!(chain, vec![TEST_DEFAULT_MODEL]);
}

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

    let cloudflare: ReasonerError = ApiError::CloudflareBlock("Cloudflare block".into()).into();
    assert!(cloudflare.to_string().contains("Cloudflare"));

    let other: ReasonerError =
        ApiError::Other(ReasonerError::Request("network error".into())).into();
    assert!(other.to_string().contains("network error"));

    // Axis 2: generic 5xx round-trips back to `ReasonerError::Api`
    // when retries are exhausted, preserving both the status and the
    // preview the dev loop already surfaces in `task_failed` reasons.
    let transient_5xx: ReasonerError = ApiError::TransientServer {
        status: 500,
        message: "Anthropic API error: 500 Internal Server Error - body".into(),
    }
    .into();
    match transient_5xx {
        ReasonerError::Api { status, ref message } => {
            assert_eq!(status, 500);
            assert!(
                message.contains("Internal Server Error"),
                "TransientServer should preserve the body preview: {message}"
            );
        }
        other => panic!("TransientServer should map to ReasonerError::Api, got {other:?}"),
    }
}

#[test]
fn test_overloaded_message_appends_retry_after_hint_when_absent() {
    let err: ReasonerError = ApiError::Overloaded {
        message: "Anthropic API error: 429 Too Many Requests - server busy".into(),
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
        message: "Anthropic API error: 429 - \
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
    use super::is_cloudflare_html;
    assert!(is_cloudflare_html(
        r#"<!DOCTYPE html><!--[if lt IE 7]> <html class="no-js ie6 oldie" lang="en-US">"#
    ));
    assert!(!is_cloudflare_html(
        r#"{"error":{"type":"authentication_error","message":"invalid api key"}}"#
    ));
}

#[test]
fn test_resolve_thinking_explicit_config() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(8192)
        .thinking(ThinkingConfig {
            budget_tokens: 4000,
        })
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, TEST_DEFAULT_MODEL);
    assert!(thinking.is_some());
    let thinking = thinking.unwrap();
    assert_eq!(thinking.thinking_type, "adaptive");
    assert_eq!(thinking.budget_tokens, None);
}

#[test]
fn test_resolve_thinking_auto_for_capable_model() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(8192)
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, TEST_DEFAULT_MODEL);
    assert!(thinking.is_some());
    let thinking = thinking.unwrap();
    assert_eq!(thinking.thinking_type, "adaptive");
    assert_eq!(thinking.budget_tokens, None);
}

#[test]
fn test_resolve_thinking_auto_for_aura_alias_capable_model() {
    let request = ModelRequest::builder("aura-claude-opus-4-7", "system")
        .max_tokens(8192)
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, "aura-claude-opus-4-7").unwrap();
    assert_eq!(thinking.thinking_type, "adaptive");
    assert_eq!(thinking.budget_tokens, None);
}

#[test]
fn test_resolve_thinking_uses_enabled_budget_for_older_models() {
    let request = ModelRequest::builder("claude-3-7-sonnet", "system")
        .max_tokens(8192)
        .thinking(ThinkingConfig {
            budget_tokens: 4000,
        })
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, "claude-3-7-sonnet").unwrap();
    assert_eq!(thinking.thinking_type, "enabled");
    assert_eq!(thinking.budget_tokens, Some(4000));
}

#[test]
fn test_resolve_thinking_none_for_unsupported_haiku_variants() {
    let request = ModelRequest::builder("aura-claude-haiku-4-5", "system")
        .max_tokens(8192)
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, "aura-claude-haiku-4-5");
    assert!(thinking.is_none());
}

#[test]
fn test_resolve_thinking_auto_for_non_claude_model_is_none() {
    let request = ModelRequest::builder("aura-gpt-5-4", "system")
        .max_tokens(8192)
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, "aura-gpt-5-4");
    assert!(thinking.is_none());
}

#[test]
fn test_resolve_thinking_none_for_small_budget() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(1024)
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, TEST_DEFAULT_MODEL);
    assert!(thinking.is_none());
}

#[test]
fn test_resolve_thinking_none_for_unsupported_claude_3_variants() {
    let request = ModelRequest::builder("claude-3-haiku", "system")
        .max_tokens(8192)
        .try_build()
        .unwrap();
    let thinking = resolve_thinking(&request, "claude-3-haiku");
    assert!(thinking.is_none());
}

#[test]
fn test_resolve_output_config_only_for_claude_4_thinking_models() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(8192)
        .try_build()
        .unwrap();
    let output = resolve_output_config(&request, TEST_DEFAULT_MODEL).unwrap();
    assert_eq!(output.effort, "high");

    let sonnet_37_output = resolve_output_config(&request, "claude-3-7-sonnet");
    assert!(sonnet_37_output.is_none());
}

#[tokio::test]
async fn test_proxy_mode_sends_caching_beta_header() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let n = socket.read(&mut buf).await.unwrap();
        let request_text = String::from_utf8_lossy(&buf[..n]).to_string();

        let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"test","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();

        request_text
    });

    let config = AnthropicConfig {
        api_key: String::new(),
        default_model: "aura-claude-sonnet-4-6".to_string(),
        timeout_ms: 5000,
        max_retries: 0,
        base_url: format!("http://127.0.0.1:{}", addr.port()),
        routing_mode: RoutingMode::Proxy,
        fallback_model: None,
        prompt_caching_enabled: true,
    };

    let provider = AnthropicProvider::new(config).unwrap();
    let request = ModelRequest::builder("aura-claude-sonnet-4-6", "system")
        .message(Message::user("test"))
        .auth_token(Some("test-jwt-token".to_string()))
        .try_build()
        .unwrap();

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
async fn test_direct_mode_sends_caching_beta_header() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let n = socket.read(&mut buf).await.unwrap();
        let request_text = String::from_utf8_lossy(&buf[..n]).to_string();

        let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"test","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();

        request_text
    });

    let config = AnthropicConfig {
        api_key: "test-api-key".to_string(),
        default_model: "test-model".to_string(),
        timeout_ms: 5000,
        max_retries: 0,
        base_url: format!("http://127.0.0.1:{}", addr.port()),
        routing_mode: RoutingMode::Direct,
        fallback_model: None,
        prompt_caching_enabled: true,
    };

    let provider = AnthropicProvider::new(config).unwrap();
    let request = ModelRequest::builder("test-model", "system")
        .message(Message::user("test"))
        .try_build()
        .unwrap();

    let _ = provider.complete(request).await;

    let captured = server.await.unwrap();
    assert!(
        captured.contains("anthropic-beta"),
        "Direct request should include anthropic-beta header.\nCaptured headers:\n{captured}"
    );
    assert!(
        captured.contains("prompt-caching-2024-07-31"),
        "anthropic-beta header should include prompt-caching beta tag.\nCaptured headers:\n{captured}"
    );
}

#[tokio::test]
async fn test_complete_timeout() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let _server = tokio::spawn(async move {
        loop {
            let Ok((_socket, _)) = listener.accept().await else {
                break;
            };
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });

    let config = AnthropicConfig {
        api_key: "test-key".to_string(),
        default_model: "test-model".to_string(),
        timeout_ms: 200,
        max_retries: 0,
        base_url: format!("http://127.0.0.1:{}", addr.port()),
        routing_mode: RoutingMode::Direct,
        fallback_model: None,
        prompt_caching_enabled: true,
    };

    let provider = AnthropicProvider::new(config).unwrap();
    let request = ModelRequest::builder("test-model", "system")
        .message(Message::user("test"))
        .try_build()
        .unwrap();

    let result = provider.complete(request).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, ReasonerError::Timeout),
        "expected Timeout, got: {err:?}"
    );
}

#[tokio::test]
async fn test_direct_mode_omits_caching_beta_header_when_disabled() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let n = socket.read(&mut buf).await.unwrap();
        let request_text = String::from_utf8_lossy(&buf[..n]).to_string();

        let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"test","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();

        request_text
    });

    let config = AnthropicConfig {
        api_key: "test-api-key".to_string(),
        default_model: "test-model".to_string(),
        timeout_ms: 5000,
        max_retries: 0,
        base_url: format!("http://127.0.0.1:{}", addr.port()),
        routing_mode: RoutingMode::Direct,
        fallback_model: None,
        prompt_caching_enabled: false,
    };

    let provider = AnthropicProvider::new(config).unwrap();
    let request = ModelRequest::builder("test-model", "system")
        .message(Message::user("test"))
        .try_build()
        .unwrap();

    let _ = provider.complete(request).await;

    let captured = server.await.unwrap();
    assert!(
        !captured.contains("anthropic-beta"),
        "Direct request should omit anthropic-beta header when prompt caching is disabled.\nCaptured headers:\n{captured}"
    );
}

#[tokio::test]
async fn test_proxy_openai_models_fall_back_to_buffered_streaming() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let n = socket.read(&mut buf).await.unwrap();
        let request_text = String::from_utf8_lossy(&buf[..n]).to_string();

        let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"proxy ok"}],"model":"aura-gpt-4.1","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();

        request_text
    });

    let config = AnthropicConfig {
        api_key: String::new(),
        default_model: "aura-gpt-4.1".to_string(),
        timeout_ms: 5000,
        max_retries: 0,
        base_url: format!("http://127.0.0.1:{}", addr.port()),
        routing_mode: RoutingMode::Proxy,
        fallback_model: None,
        prompt_caching_enabled: true,
    };

    let provider = AnthropicProvider::new(config).unwrap();
    let request = ModelRequest::builder("aura-gpt-4.1", "system")
        .message(Message::user("test"))
        .auth_token(Some("test-jwt-token".to_string()))
        .try_build()
        .unwrap();

    let stream = provider.complete_streaming(request).await.unwrap();
    let events = stream.collect::<Vec<_>>().await;

    let captured = server.await.unwrap();
    assert!(
        !captured.contains(r#""stream":true"#),
        "Buffered fallback should avoid Anthropic SSE requests.\nCaptured request:\n{captured}"
    );

    assert!(matches!(
        events.first().unwrap().as_ref().unwrap(),
        StreamEvent::MessageStart { model, .. } if model == "aura-gpt-4.1"
    ));
    assert!(events.iter().any(|event| matches!(
        event.as_ref().unwrap(),
        StreamEvent::TextDelta { text } if text == "proxy ok"
    )));
    assert!(events.iter().any(|event| matches!(
        event.as_ref().unwrap(),
        StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            output_tokens: 5,
        }
    )));
    assert!(matches!(
        events.last().unwrap().as_ref().unwrap(),
        StreamEvent::MessageStop
    ));
}

#[tokio::test]
async fn test_cross_family_proxy_fallback_buffers_streaming_and_omits_anthropic_headers() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut first_socket, _) = listener.accept().await.unwrap();
        let mut first_buf = vec![0u8; 8192];
        let first_n = first_socket.read(&mut first_buf).await.unwrap();
        let first_request = String::from_utf8_lossy(&first_buf[..first_n]).to_string();
        let overloaded = "HTTP/1.1 529 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: 35\r\n\r\n{\"type\":\"error\",\"error\":\"overloaded\"}";
        first_socket.write_all(overloaded.as_bytes()).await.unwrap();

        let (mut second_socket, _) = listener.accept().await.unwrap();
        let mut second_buf = vec![0u8; 8192];
        let second_n = second_socket.read(&mut second_buf).await.unwrap();
        let second_request = String::from_utf8_lossy(&second_buf[..second_n]).to_string();
        let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"fallback ok"}],"model":"aura-gpt-5-4","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        second_socket.write_all(response.as_bytes()).await.unwrap();

        (first_request, second_request)
    });

    let config = AnthropicConfig {
        api_key: String::new(),
        default_model: "claude-opus-4-6".to_string(),
        timeout_ms: 5000,
        max_retries: 0,
        base_url: format!("http://127.0.0.1:{}", addr.port()),
        routing_mode: RoutingMode::Proxy,
        fallback_model: Some("aura-gpt-5-4".to_string()),
        prompt_caching_enabled: true,
    };

    let provider = AnthropicProvider::new(config).unwrap();
    let request = ModelRequest::builder("claude-opus-4-6", "system")
        .message(Message::user("test"))
        .max_tokens(8192)
        .auth_token(Some("test-jwt-token".to_string()))
        .try_build()
        .unwrap();

    let stream = provider.complete_streaming(request).await.unwrap();
    let events = stream.collect::<Vec<_>>().await;

    let (first_request, second_request) = server.await.unwrap();
    assert!(
        first_request.contains(r#""stream":true"#),
        "Primary Anthropic request should still use SSE.\nCaptured request:\n{first_request}"
    );
    assert!(
        !second_request.contains(r#""stream":true"#),
        "Cross-family fallback should buffer instead of using Anthropic SSE.\nCaptured request:\n{second_request}"
    );
    assert!(
        !second_request.contains("anthropic-beta"),
        "Cross-family fallback should omit Anthropic beta headers.\nCaptured request:\n{second_request}"
    );
    assert!(
        !second_request.contains(r#""thinking":"#),
        "Cross-family fallback should omit Anthropic thinking config.\nCaptured request:\n{second_request}"
    );
    assert!(
        !second_request.contains("output_config"),
        "Cross-family fallback should omit Anthropic output config.\nCaptured request:\n{second_request}"
    );
    assert!(events.iter().any(|event| matches!(
        event.as_ref().unwrap(),
        StreamEvent::TextDelta { text } if text == "fallback ok"
    )));
}

#[tokio::test]
async fn test_proxy_openai_models_omit_prompt_caching_headers_and_fields() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let n = socket.read(&mut buf).await.unwrap();
        let request_text = String::from_utf8_lossy(&buf[..n]).to_string();

        let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"aura-gpt-4.1","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();

        request_text
    });

    let config = AnthropicConfig {
        api_key: String::new(),
        default_model: "aura-gpt-4.1".to_string(),
        timeout_ms: 5000,
        max_retries: 0,
        base_url: format!("http://127.0.0.1:{}", addr.port()),
        routing_mode: RoutingMode::Proxy,
        fallback_model: None,
        prompt_caching_enabled: true,
    };

    let provider = AnthropicProvider::new(config).unwrap();
    let request = ModelRequest::builder("aura-gpt-4.1", "system")
        .message(Message::user("test"))
        .auth_token(Some("test-jwt-token".to_string()))
        .try_build()
        .unwrap();

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
async fn test_proxy_hint_prefers_anthropic_family_over_model_heuristics() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let n = socket.read(&mut buf).await.unwrap();
        let request_text = String::from_utf8_lossy(&buf[..n]).to_string();

        let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"aura-gpt-4.1","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();

        request_text
    });

    let config = AnthropicConfig {
        api_key: String::new(),
        default_model: "aura-gpt-4.1".to_string(),
        timeout_ms: 5000,
        max_retries: 0,
        base_url: format!("http://127.0.0.1:{}", addr.port()),
        routing_mode: RoutingMode::Proxy,
        fallback_model: None,
        prompt_caching_enabled: true,
    };

    let provider = AnthropicProvider::new(config).unwrap();
    let request = ModelRequest::builder("aura-gpt-4.1", "system")
        .message(Message::user("test"))
        .auth_token(Some("test-jwt-token".to_string()))
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
async fn test_proxy_hint_prefers_non_anthropic_family_over_model_heuristics_for_streaming() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let n = socket.read(&mut buf).await.unwrap();
        let request_text = String::from_utf8_lossy(&buf[..n]).to_string();

        let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"proxy ok"}],"model":"claude-opus-4-6","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();

        request_text
    });

    let config = AnthropicConfig {
        api_key: String::new(),
        default_model: "claude-opus-4-6".to_string(),
        timeout_ms: 5000,
        max_retries: 0,
        base_url: format!("http://127.0.0.1:{}", addr.port()),
        routing_mode: RoutingMode::Proxy,
        fallback_model: None,
        prompt_caching_enabled: true,
    };

    let provider = AnthropicProvider::new(config).unwrap();
    let request = ModelRequest::builder("claude-opus-4-6", "system")
        .message(Message::user("test"))
        .auth_token(Some("test-jwt-token".to_string()))
        .upstream_provider_family(Some("openai".to_string()))
        .try_build()
        .unwrap();

    let stream = provider.complete_streaming(request).await.unwrap();
    let events = stream.collect::<Vec<_>>().await;

    let captured = server.await.unwrap();
    assert!(
        !captured.contains(r#""stream":true"#),
        "Explicit non-Anthropic family hints should force buffered proxy streaming even for Claude model names.\nCaptured request:\n{captured}"
    );
    assert!(
        !captured.contains("anthropic-beta"),
        "Explicit non-Anthropic family hints should suppress Anthropic proxy headers even for Claude model names.\nCaptured request:\n{captured}"
    );
    assert!(
        !captured.contains(r#""thinking":"#),
        "Explicit non-Anthropic family hints should suppress Anthropic thinking config even for Claude model names.\nCaptured request:\n{captured}"
    );
    assert!(
        !captured.contains("output_config"),
        "Explicit non-Anthropic family hints should suppress Anthropic output config even for Claude model names.\nCaptured request:\n{captured}"
    );
    assert!(matches!(
        events.first().unwrap().as_ref().unwrap(),
        StreamEvent::MessageStart { model, .. } if model == "claude-opus-4-6"
    ));
}

#[tokio::test]
async fn test_proxy_non_anthropic_family_omits_thinking_and_output_config_for_complete() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let n = socket.read(&mut buf).await.unwrap();
        let request_text = String::from_utf8_lossy(&buf[..n]).to_string();

        let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"claude-opus-4-6","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();

        request_text
    });

    let config = AnthropicConfig {
        api_key: String::new(),
        default_model: "claude-opus-4-6".to_string(),
        timeout_ms: 5000,
        max_retries: 0,
        base_url: format!("http://127.0.0.1:{}", addr.port()),
        routing_mode: RoutingMode::Proxy,
        fallback_model: None,
        prompt_caching_enabled: true,
    };

    let provider = AnthropicProvider::new(config).unwrap();
    let request = ModelRequest::builder("claude-opus-4-6", "system")
        .message(Message::user("test"))
        .max_tokens(8192)
        .auth_token(Some("test-jwt-token".to_string()))
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
