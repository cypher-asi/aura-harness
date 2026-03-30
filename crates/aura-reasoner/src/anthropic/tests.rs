use super::api_types::{ApiContent, ApiToolChoice};
use super::convert::{
    build_system_block, convert_messages_to_api, convert_tool_choice, convert_tools_to_api,
    resolve_thinking,
};
use super::{AnthropicConfig, AnthropicProvider, ApiError, RoutingMode};
use crate::{
    Message, ModelProvider, ModelRequest, ReasonerError, ThinkingConfig, ToolChoice, ToolDefinition,
};

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

    let api_msgs = convert_messages_to_api(&messages);
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

    let api_tools = convert_tools_to_api(&tools);
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
    let system = build_system_block("You are a helpful assistant.");
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

    let api_tools = convert_tools_to_api(&tools);
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

    let api_msgs = convert_messages_to_api(&messages);

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

    let system = build_system_block("test");
    let json = serde_json::to_string(&system).unwrap();
    assert!(json.contains("cache_control"));
    assert!(json.contains("ephemeral"));

    assert_eq!(provider.name(), "anthropic");
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
    let overloaded: ReasonerError = ApiError::Overloaded("529 overloaded".into()).into();
    assert!(overloaded.to_string().contains("529"));

    let credits: ReasonerError = ApiError::InsufficientCredits("402 insufficient".into()).into();
    assert!(credits.to_string().contains("402"));

    let cloudflare: ReasonerError = ApiError::CloudflareBlock("Cloudflare block".into()).into();
    assert!(cloudflare.to_string().contains("Cloudflare"));

    let other: ReasonerError =
        ApiError::Other(ReasonerError::Request("network error".into())).into();
    assert!(other.to_string().contains("network error"));
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
        .build();
    let thinking = resolve_thinking(&request, TEST_DEFAULT_MODEL);
    assert!(thinking.is_some());
    assert_eq!(thinking.unwrap().budget_tokens, 4000);
}

#[test]
fn test_resolve_thinking_auto_for_capable_model() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(8192)
        .build();
    let thinking = resolve_thinking(&request, TEST_DEFAULT_MODEL);
    assert!(thinking.is_some());
    assert_eq!(thinking.unwrap().budget_tokens, 4096);
}

#[test]
fn test_resolve_thinking_none_for_small_budget() {
    let request = ModelRequest::builder(TEST_DEFAULT_MODEL, "system")
        .max_tokens(1024)
        .build();
    let thinking = resolve_thinking(&request, TEST_DEFAULT_MODEL);
    assert!(thinking.is_none());
}

#[test]
fn test_resolve_thinking_none_for_unsupported_model() {
    let request = ModelRequest::builder("claude-3-haiku", "system")
        .max_tokens(8192)
        .build();
    let thinking = resolve_thinking(&request, "claude-3-haiku");
    assert!(thinking.is_none());
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
        default_model: "test-model".to_string(),
        timeout_ms: 5000,
        max_retries: 0,
        base_url: format!("http://127.0.0.1:{}", addr.port()),
        routing_mode: RoutingMode::Proxy,
        fallback_model: None,
    };

    let provider = AnthropicProvider::new(config).unwrap();
    let request = ModelRequest::builder("test-model", "system")
        .message(Message::user("test"))
        .auth_token(Some("test-jwt-token".to_string()))
        .build();

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
    };

    let provider = AnthropicProvider::new(config).unwrap();
    let request = ModelRequest::builder("test-model", "system")
        .message(Message::user("test"))
        .build();

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
