//! Anthropic provider implementation.
//!
//! Uses `reqwest` directly to communicate with the Anthropic API.
//! This approach is more reliable than SDK wrappers which may have
//! incompatible or private internal types.
//!
//! Supports both synchronous and streaming completions via SSE.

mod api_types;
mod config;
mod convert;
mod sse;

pub use config::{AnthropicConfig, RoutingMode};

use api_types::{ApiRequest, ApiResponse, StreamingApiRequest};
use convert::{
    build_system_block, convert_messages_to_api, convert_response_to_aura, convert_tool_choice,
    convert_tools_to_api, resolve_thinking,
};
use sse::SseStream;

use crate::error::ReasonerError;
use crate::{
    ModelProvider, ModelRequest, ModelResponse, ProviderTrace, StopReason, StreamEventStream, Usage,
};
use async_trait::async_trait;
use serde::Serialize;
use std::time::Instant;
use tracing::{debug, error, info, instrument, warn};

// ============================================================================
// Internal Error Classification (for retry logic)
// ============================================================================

#[derive(Debug)]
enum ApiError {
    /// 429 / 529 — retryable with backoff, then fallback.
    Overloaded(String),
    /// 402 — stop immediately, no retry or fallback.
    InsufficientCredits(String),
    /// 403 / 503 with Cloudflare HTML — retryable (service cold-starting).
    CloudflareBlock(String),
    /// Any other failure.
    Other(ReasonerError),
}

impl From<ApiError> for ReasonerError {
    fn from(e: ApiError) -> Self {
        match e {
            ApiError::Overloaded(msg) => ReasonerError::RateLimited(msg),
            ApiError::InsufficientCredits(msg) => ReasonerError::InsufficientCredits(msg),
            ApiError::CloudflareBlock(msg) => ReasonerError::Api {
                status: 403,
                message: msg,
            },
            ApiError::Other(e) => e,
        }
    }
}

fn is_cloudflare_html(body: &str) -> bool {
    body.contains("<!DOCTYPE html") && (body.contains("cloudflare") || body.contains("oldie"))
}

// ============================================================================
// Provider Implementation
// ============================================================================

/// Anthropic model provider.
///
/// Implements `ModelProvider` for the Anthropic API using direct HTTP calls.
/// Includes built-in retry with exponential backoff for overloaded (429/529)
/// errors and automatic fallback to a secondary model.
pub struct AnthropicProvider {
    client: reqwest::Client,
    config: AnthropicConfig,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider.
    ///
    /// # Errors
    ///
    /// Returns error if client creation fails.
    pub fn new(config: AnthropicConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_millis(config.timeout_ms))
            .build()?;
        Ok(Self { client, config })
    }

    /// Create from environment variables.
    ///
    /// # Errors
    ///
    /// Returns error if configuration or client creation fails.
    pub fn from_env() -> anyhow::Result<Self> {
        let config = AnthropicConfig::from_env()?;
        Self::new(config)
    }

    /// Build the ordered model fallback chain.
    fn model_chain(&self, primary: &str) -> Vec<String> {
        let mut models = vec![primary.to_string()];
        if let Some(ref fb) = self.config.fallback_model {
            if fb != primary {
                models.push(fb.clone());
            }
        }
        models
    }

    /// Send an HTTP request to the Anthropic API and classify the response.
    ///
    /// Returns the raw `reqwest::Response` on success, or an [`ApiError`]
    /// that the retry loop can pattern-match on.
    async fn send_checked<B: Serialize + Sync>(
        &self,
        request_ctx: &ModelRequest,
        json_body: &B,
    ) -> Result<reqwest::Response, ApiError> {
        let mut req_builder = self
            .client
            .post(format!("{}/v1/messages", self.config.base_url))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(json_body);

        match self.config.routing_mode {
            RoutingMode::Direct => {
                req_builder = req_builder
                    .header("x-api-key", &self.config.api_key)
                    .header("anthropic-beta", "prompt-caching-2024-07-31");
            }
            RoutingMode::Proxy => {
                let token = request_ctx.auth_token.as_deref().ok_or_else(|| {
                    ApiError::Other(ReasonerError::Internal(
                        "Proxy mode requires a JWT auth token".into(),
                    ))
                })?;
                req_builder = req_builder
                    .header("authorization", format!("Bearer {token}"))
                    .header("anthropic-beta", "prompt-caching-2024-07-31");
                if let Some(ref v) = request_ctx.aura_project_id {
                    req_builder = req_builder.header("X-Aura-Project-Id", v);
                }
                if let Some(ref v) = request_ctx.aura_agent_id {
                    req_builder = req_builder.header("X-Aura-Agent-Id", v);
                }
                if let Some(ref v) = request_ctx.aura_session_id {
                    req_builder = req_builder.header("X-Aura-Session-Id", v);
                }
                if let Some(ref v) = request_ctx.aura_org_id {
                    req_builder = req_builder.header("X-Aura-Org-Id", v);
                }
            }
        }

        let response = req_builder.send().await.map_err(|e| {
            error!(error = %e, "Anthropic API request failed");
            ApiError::Other(ReasonerError::Request(format!(
                "Anthropic API request failed: {e}"
            )))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let status_code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            let body_preview = crate::truncate_body(&body, 200);
            error!(status = %status, body = %body_preview, "Anthropic API error");

            if is_cloudflare_html(&body) {
                return Err(ApiError::CloudflareBlock(format!(
                    "LLM proxy returned Cloudflare block ({status}) — service may be cold-starting"
                )));
            }

            return match status_code {
                402 => Err(ApiError::InsufficientCredits(format!(
                    "Anthropic API error: {status} - {body}"
                ))),
                429 | 529 => Err(ApiError::Overloaded(format!(
                    "Anthropic API error: {status} - {body}"
                ))),
                _ => Err(ApiError::Other(ReasonerError::Api {
                    status: status_code,
                    message: format!("{status} - {body}"),
                })),
            };
        }

        Ok(response)
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    #[instrument(skip(self, request), fields(model = %request.model))]
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        let start = Instant::now();
        let models = self.model_chain(&request.model);
        let system = build_system_block(&request.system);

        let mut last_err: Option<ReasonerError> = None;

        for (model_idx, model) in models.iter().enumerate() {
            let thinking = resolve_thinking(&request, model);
            let api_request = ApiRequest {
                model: model.clone(),
                system: system.clone(),
                messages: convert_messages_to_api(&request.messages),
                tools: if request.tools.is_empty() {
                    None
                } else {
                    Some(convert_tools_to_api(&request.tools))
                },
                tool_choice: convert_tool_choice(&request.tool_choice),
                max_tokens: request.max_tokens,
                temperature: if thinking.is_some() {
                    Some(1.0)
                } else {
                    request.temperature
                },
                thinking,
            };

            debug!(
                model = %model,
                messages = api_request.messages.len(),
                tools = api_request.tools.as_ref().map_or(0, Vec::len),
                "Sending request to Anthropic"
            );

            for attempt in 0..=self.config.max_retries {
                if attempt > 0 {
                    let backoff_ms = 1000 * u64::from(2u32.pow(attempt - 1));
                    warn!(attempt, model = %model, backoff_ms, "Retrying after overloaded error");
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                }

                match self.send_checked(&request, &api_request).await {
                    Ok(response) => {
                        let latency_ms =
                            u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

                        let api_response: ApiResponse = response.json().await.map_err(|e| {
                            error!(error = %e, "Failed to parse Anthropic response");
                            ReasonerError::Parse(format!("Failed to parse Anthropic response: {e}"))
                        })?;

                        let message = convert_response_to_aura(&api_response.content);
                        let stop_reason = match api_response.stop_reason.as_deref() {
                            Some("tool_use") => StopReason::ToolUse,
                            Some("max_tokens") => StopReason::MaxTokens,
                            Some("stop_sequence") => StopReason::StopSequence,
                            _ => StopReason::EndTurn,
                        };

                        if model_idx > 0 {
                            info!(
                                primary = %request.model,
                                fallback = %model,
                                "Completed with fallback model"
                            );
                        }

                        debug!(
                            stop_reason = ?stop_reason,
                            latency_ms,
                            input_tokens = api_response.usage.input_tokens,
                            output_tokens = api_response.usage.output_tokens,
                            model_used = %model,
                            "Received response from Anthropic"
                        );

                        let model_used = api_response.model.clone();

                        return Ok(ModelResponse {
                            stop_reason,
                            message,
                            usage: Usage {
                                input_tokens: api_response.usage.input_tokens,
                                output_tokens: api_response.usage.output_tokens,
                                cache_creation_input_tokens: api_response
                                    .usage
                                    .cache_creation_input_tokens,
                                cache_read_input_tokens: api_response.usage.cache_read_input_tokens,
                            },
                            trace: ProviderTrace {
                                request_id: Some(api_response.id),
                                latency_ms,
                                model: api_response.model,
                            },
                            model_used,
                        });
                    }
                    Err(ApiError::InsufficientCredits(msg)) => {
                        return Err(ReasonerError::InsufficientCredits(msg));
                    }
                    Err(ApiError::CloudflareBlock(ref msg))
                        if attempt < self.config.max_retries =>
                    {
                        warn!(model = %model, attempt, "Cloudflare block, will retry");
                        last_err = Some(ReasonerError::Api {
                            status: 403,
                            message: msg.clone(),
                        });
                    }
                    Err(ApiError::Overloaded(ref msg)) if attempt < self.config.max_retries => {
                        warn!(model = %model, attempt, "API overloaded, will retry");
                        last_err = Some(ReasonerError::RateLimited(msg.clone()));
                    }
                    Err(ApiError::Overloaded(ref msg)) if model_idx < models.len() - 1 => {
                        warn!(model = %model, "Retries exhausted, falling back to next model");
                        last_err = Some(ReasonerError::RateLimited(msg.clone()));
                        break;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ReasonerError::Internal("All models in fallback chain exhausted".into())
        }))
    }

    /// Health is determined lazily on first request; the Anthropic API does
    /// not expose a dedicated health-check endpoint, so we optimistically
    /// return `true` and let request-time errors drive retry/fallback.
    async fn health_check(&self) -> bool {
        true
    }

    #[instrument(skip(self, request), fields(model = %request.model))]
    async fn complete_streaming(
        &self,
        request: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        let models = self.model_chain(&request.model);
        let system = build_system_block(&request.system);

        let mut last_err: Option<ReasonerError> = None;

        for (model_idx, model) in models.iter().enumerate() {
            let thinking = resolve_thinking(&request, model);
            let api_request = StreamingApiRequest {
                model: model.clone(),
                system: system.clone(),
                messages: convert_messages_to_api(&request.messages),
                tools: if request.tools.is_empty() {
                    None
                } else {
                    Some(convert_tools_to_api(&request.tools))
                },
                tool_choice: convert_tool_choice(&request.tool_choice),
                max_tokens: request.max_tokens,
                temperature: if thinking.is_some() {
                    Some(1.0)
                } else {
                    request.temperature
                },
                stream: true,
                thinking,
            };

            debug!(
                model = %model,
                messages = api_request.messages.len(),
                tools = api_request.tools.as_ref().map_or(0, Vec::len),
                "Sending streaming request to Anthropic"
            );

            for attempt in 0..=self.config.max_retries {
                if attempt > 0 {
                    let backoff_ms = 1000 * u64::from(2u32.pow(attempt - 1));
                    warn!(attempt, model = %model, backoff_ms, "Retrying streaming after overloaded error");
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                }

                match self.send_checked(&request, &api_request).await {
                    Ok(response) => {
                        if model_idx > 0 {
                            info!(
                                primary = %request.model,
                                fallback = %model,
                                "Streaming with fallback model"
                            );
                        }
                        let byte_stream = response.bytes_stream();
                        let sse_stream = SseStream::new(byte_stream);
                        return Ok(Box::pin(sse_stream));
                    }
                    Err(ApiError::InsufficientCredits(msg)) => {
                        return Err(ReasonerError::InsufficientCredits(msg));
                    }
                    Err(ApiError::CloudflareBlock(ref msg))
                        if attempt < self.config.max_retries =>
                    {
                        warn!(model = %model, attempt, "Streaming Cloudflare block, will retry");
                        last_err = Some(ReasonerError::Api {
                            status: 403,
                            message: msg.clone(),
                        });
                    }
                    Err(ApiError::Overloaded(ref msg)) if attempt < self.config.max_retries => {
                        warn!(model = %model, attempt, "Streaming API overloaded, will retry");
                        last_err = Some(ReasonerError::RateLimited(msg.clone()));
                    }
                    Err(ApiError::Overloaded(ref msg)) if model_idx < models.len() - 1 => {
                        warn!(model = %model, "Streaming retries exhausted, falling back");
                        last_err = Some(ReasonerError::RateLimited(msg.clone()));
                        break;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ReasonerError::Internal("All models in fallback chain exhausted".into())
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::api_types::{ApiContent, ApiToolChoice};
    use super::convert::{
        build_system_block, convert_messages_to_api, convert_tool_choice, convert_tools_to_api,
        resolve_thinking,
    };
    use super::{AnthropicConfig, AnthropicProvider, ApiError, RoutingMode};
    use crate::{
        Message, ModelProvider, ModelRequest, ReasonerError, ThinkingConfig, ToolChoice,
        ToolDefinition,
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

        let credits: ReasonerError =
            ApiError::InsufficientCredits("402 insufficient".into()).into();
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
}
