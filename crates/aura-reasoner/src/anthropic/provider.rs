use super::api_types::{ApiRequest, ApiResponse, StreamingApiRequest};
use super::convert::{
    build_system_block, convert_messages_to_api, convert_response_to_aura, convert_tool_choice,
    convert_tools_to_api, resolve_thinking,
};
use super::sse::SseStream;
use super::{AnthropicProvider, ApiError};

use crate::error::ReasonerError;
use crate::{
    stream_from_response, ModelProvider, ModelRequest, ModelResponse, ProviderTrace, StopReason,
    StreamEventStream, Usage,
};
use async_trait::async_trait;
use serde::Serialize;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

impl AnthropicProvider {
    fn model_looks_like_anthropic(model: &str) -> bool {
        let model = model.trim().to_ascii_lowercase();
        model.starts_with("claude") || model.starts_with("aura-claude")
    }

    fn supports_anthropic_proxy_features(request: &ModelRequest, model: &str) -> bool {
        if let Some(family) = request
            .upstream_provider_family
            .as_deref()
            .map(str::trim)
            .filter(|family| !family.is_empty())
        {
            return family.eq_ignore_ascii_case("anthropic");
        }

        Self::model_looks_like_anthropic(model)
    }

    fn prompt_caching_enabled_for_model(&self, request: &ModelRequest, model: &str) -> bool {
        self.config.prompt_caching_enabled
            && match self.config.routing_mode {
                super::RoutingMode::Proxy => {
                    Self::supports_anthropic_proxy_features(request, model)
                }
                super::RoutingMode::Direct => Self::model_looks_like_anthropic(model),
            }
    }

    fn should_buffer_proxy_streaming(request: &ModelRequest) -> bool {
        !Self::supports_anthropic_proxy_features(request, &request.model)
    }

    async fn check_base_url_reachable(&self) -> bool {
        let ping_url = format!("{}/", self.config.base_url.trim_end_matches('/'));
        let result = self
            .client
            .get(ping_url)
            .timeout(Duration::from_secs(5))
            .send()
            .await;

        match result {
            Ok(resp) => {
                let status = resp.status();
                status.is_success()
                    || status.is_client_error()
                    || status.is_server_error()
                    || status.is_redirection()
            }
            Err(e) => {
                warn!(error = %e, "Anthropic health check failed");
                false
            }
        }
    }

    /// Send an HTTP request to the Anthropic API and classify the response.
    pub(super) async fn send_checked<B: Serialize + Sync>(
        &self,
        request_ctx: &ModelRequest,
        json_body: &B,
    ) -> Result<reqwest::Response, ApiError> {
        let req_builder = self.build_request(request_ctx, json_body)?;

        let response = req_builder.send().await.map_err(|e| {
            error!(error = %e, "Anthropic API request failed");
            if e.is_timeout() {
                ApiError::Other(ReasonerError::Timeout)
            } else {
                ApiError::Other(ReasonerError::Request(format!(
                    "Anthropic API request failed: {e}"
                )))
            }
        })?;

        if !response.status().is_success() {
            return Err(classify_api_error(response).await);
        }

        Ok(response)
    }

    fn build_request<B: Serialize + Sync>(
        &self,
        request_ctx: &ModelRequest,
        json_body: &B,
    ) -> Result<reqwest::RequestBuilder, ApiError> {
        use super::config::RoutingMode;

        let mut req_builder = self
            .client
            .post(format!("{}/v1/messages", self.config.base_url))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(json_body);
        let prompt_caching_enabled = self.config.prompt_caching_enabled;
        let proxy_prompt_caching_enabled = prompt_caching_enabled
            && Self::supports_anthropic_proxy_features(request_ctx, &request_ctx.model);

        match self.config.routing_mode {
            RoutingMode::Direct => {
                req_builder = req_builder.header("x-api-key", &self.config.api_key);
                if prompt_caching_enabled {
                    req_builder = req_builder.header("anthropic-beta", "prompt-caching-2024-07-31");
                }
            }
            RoutingMode::Proxy => {
                let token = request_ctx.auth_token.as_deref().ok_or_else(|| {
                    ApiError::Other(ReasonerError::Internal(
                        "Proxy mode requires a JWT auth token".into(),
                    ))
                })?;
                req_builder = req_builder.header("authorization", format!("Bearer {token}"));
                if proxy_prompt_caching_enabled {
                    req_builder = req_builder.header("anthropic-beta", "prompt-caching-2024-07-31");
                }
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

        Ok(req_builder)
    }
}

async fn classify_api_error(response: reqwest::Response) -> ApiError {
    let status = response.status();
    let status_code = status.as_u16();
    let body = response.text().await.unwrap_or_default();
    let body_preview = crate::truncate_body(&body, 200);
    error!(status = %status, body = %body_preview, "Anthropic API error");

    if super::is_cloudflare_html(&body) {
        return ApiError::CloudflareBlock(format!(
            "LLM proxy returned Cloudflare block ({status}) — service may be cold-starting"
        ));
    }

    match status_code {
        402 => ApiError::InsufficientCredits(format!("Anthropic API error: {status} - {body}")),
        429 | 529 => ApiError::Overloaded(format!("Anthropic API error: {status} - {body}")),
        _ => ApiError::Other(ReasonerError::Api {
            status: status_code,
            message: format!("{status} - {body}"),
        }),
    }
}

fn build_api_request(
    request: &ModelRequest,
    model: &str,
    system: &serde_json::Value,
    prompt_caching_enabled: bool,
) -> ApiRequest {
    let thinking = resolve_thinking(request, model);
    ApiRequest {
        model: model.to_string(),
        system: system.clone(),
        messages: convert_messages_to_api(&request.messages, prompt_caching_enabled),
        tools: if request.tools.is_empty() {
            None
        } else {
            Some(convert_tools_to_api(&request.tools, prompt_caching_enabled))
        },
        tool_choice: convert_tool_choice(&request.tool_choice),
        max_tokens: request.max_tokens,
        temperature: if thinking.is_some() {
            Some(1.0)
        } else {
            request.temperature
        },
        thinking,
    }
}

fn parse_complete_response(
    api_response: ApiResponse,
    model_idx: usize,
    request_model: &str,
    model: &str,
    latency_ms: u64,
) -> ModelResponse {
    let message = convert_response_to_aura(&api_response.content);
    let stop_reason = match api_response.stop_reason.as_deref() {
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("stop_sequence") => StopReason::StopSequence,
        _ => StopReason::EndTurn,
    };

    if model_idx > 0 {
        info!(primary = %request_model, fallback = %model, "Completed with fallback model");
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

    ModelResponse {
        stop_reason,
        message,
        usage: Usage {
            input_tokens: api_response.usage.input_tokens,
            output_tokens: api_response.usage.output_tokens,
            cache_creation_input_tokens: api_response.usage.cache_creation_input_tokens,
            cache_read_input_tokens: api_response.usage.cache_read_input_tokens,
        },
        trace: ProviderTrace {
            request_id: Some(api_response.id),
            latency_ms,
            model: api_response.model,
        },
        model_used,
    }
}

/// Handle retry errors — returns `Ok(true)` to break the inner retry loop (fallback),
/// `Ok(false)` to continue retrying, `Err` to propagate immediately.
/// Handle retry errors — returns `Ok(Some(true))` to break inner retry loop (fallback),
/// `Ok(Some(false))` to continue retrying, `Ok(None)` to propagate the error directly.
/// In the `None` case, the caller must convert the original `ApiError` into `ReasonerError`.
fn classify_retry_action(
    err: &ApiError,
    attempt: u32,
    max_retries: u32,
    model_idx: usize,
    model_count: usize,
    model: &str,
    last_err: &mut Option<ReasonerError>,
) -> Option<bool> {
    match err {
        ApiError::CloudflareBlock(msg) if attempt < max_retries => {
            warn!(model = %model, attempt, "Cloudflare block, will retry");
            *last_err = Some(ReasonerError::Api {
                status: 403,
                message: msg.clone(),
            });
            Some(false)
        }
        ApiError::Overloaded(msg) if attempt < max_retries => {
            warn!(model = %model, attempt, "API overloaded, will retry");
            *last_err = Some(ReasonerError::RateLimited(msg.clone()));
            Some(false)
        }
        ApiError::Overloaded(msg) if model_idx < model_count - 1 => {
            warn!(model = %model, "Retries exhausted, falling back to next model");
            *last_err = Some(ReasonerError::RateLimited(msg.clone()));
            Some(true)
        }
        _ => None,
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    #[tracing::instrument(skip(self, request), fields(model = %request.model))]
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        let start = Instant::now();
        let models = self.model_chain(&request.model);
        let mut last_err: Option<ReasonerError> = None;

        for (model_idx, model) in models.iter().enumerate() {
            let prompt_caching_enabled = self.prompt_caching_enabled_for_model(&request, model);
            let system = build_system_block(&request.system, prompt_caching_enabled);
            let api_request = build_api_request(&request, model, &system, prompt_caching_enabled);

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
                        return Ok(parse_complete_response(
                            api_response,
                            model_idx,
                            &request.model,
                            model,
                            latency_ms,
                        ));
                    }
                    Err(e) => {
                        match classify_retry_action(
                            &e,
                            attempt,
                            self.config.max_retries,
                            model_idx,
                            models.len(),
                            model,
                            &mut last_err,
                        ) {
                            Some(true) => break,
                            Some(false) => {}
                            None => return Err(e.into()),
                        }
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ReasonerError::Internal("All models in fallback chain exhausted".into())
        }))
    }

    async fn health_check(&self) -> bool {
        use super::config::RoutingMode;
        match self.config.routing_mode {
            RoutingMode::Direct if self.config.api_key.trim().is_empty() => false,
            _ => self.check_base_url_reachable().await,
        }
    }

    #[tracing::instrument(skip(self, request), fields(model = %request.model))]
    async fn complete_streaming(
        &self,
        request: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        if self.config.routing_mode == super::RoutingMode::Proxy
            && Self::should_buffer_proxy_streaming(&request)
        {
            debug!(
                model = %request.model,
                "Proxy-backed model does not support Anthropic SSE; buffering completion"
            );
            let response = self.complete(request).await?;
            return Ok(stream_from_response(response));
        }

        let models = self.model_chain(&request.model);
        let mut last_err: Option<ReasonerError> = None;

        for (model_idx, model) in models.iter().enumerate() {
            let prompt_caching_enabled = self.prompt_caching_enabled_for_model(&request, model);
            let system = build_system_block(&request.system, prompt_caching_enabled);
            let thinking = resolve_thinking(&request, model);
            let api_request = StreamingApiRequest {
                model: model.clone(),
                system: system.clone(),
                messages: convert_messages_to_api(&request.messages, prompt_caching_enabled),
                tools: if request.tools.is_empty() {
                    None
                } else {
                    Some(convert_tools_to_api(&request.tools, prompt_caching_enabled))
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
                    Err(e) => {
                        match classify_retry_action(
                            &e,
                            attempt,
                            self.config.max_retries,
                            model_idx,
                            models.len(),
                            model,
                            &mut last_err,
                        ) {
                            Some(true) => break,
                            Some(false) => {}
                            None => return Err(e.into()),
                        }
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ReasonerError::Internal("All models in fallback chain exhausted".into())
        }))
    }
}
