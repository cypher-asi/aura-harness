use super::errors::{classify_transport_error, is_retryable_transport_error};
use super::request::{build_api_request, parse_complete_response};
use super::retry::run_model_chain_with_retries;
use crate::anthropic::api_types::{ApiResponse, StreamingApiRequest};
use crate::anthropic::convert::{
    build_request_system, convert_messages_to_api, convert_tool_entries_to_api,
    resolve_output_config, resolve_thinking, system_salt, tool_choice_for,
};
use crate::anthropic::origin_tag::{tag_response, tag_stream};
use crate::anthropic::sse::SseStream;
use crate::anthropic::{AnthropicProvider, ApiError};
use crate::error::ReasonerError;
use crate::{
    response_output_shape, stream_from_response, ModelProvider, ModelRequest, ModelResponse,
    StreamEventStream,
};
use async_trait::async_trait;
use aura_core_types::ErrorChain;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    #[tracing::instrument(skip(self, request), fields(model = %request.model))]
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        let start = Instant::now();
        let models = self.model_chain(request.model.as_ref());
        let request_ref = &request;

        run_model_chain_with_retries(&self.config, &models, |ctx, model| {
            Box::pin(async move {
                let prompt_caching_enabled =
                    self.prompt_caching_enabled_for_model(request_ref, &model);
                let anthropic_features_enabled =
                    self.anthropic_request_features_enabled(request_ref, &model);
                let openai_features_enabled =
                    Self::supports_openai_proxy_features(request_ref, &model);
                let system = build_request_system(
                    request_ref,
                    prompt_caching_enabled,
                    system_salt(request_ref, openai_features_enabled),
                );
                let (openai_cache_key, openai_cache_retention) = if openai_features_enabled {
                    (
                        // Already clamped to OpenAI's 64-char limit when the
                        // `ModelRequest` was built (see `ModelRequestBuilder::try_build`).
                        request_ref.prompt_cache_key.clone(),
                        request_ref
                            .prompt_cache_retention
                            .map(crate::types::PromptCacheRetention::as_wire),
                    )
                } else {
                    (None, None)
                };
                let mut api_request = build_api_request(
                    request_ref,
                    &model,
                    system.as_ref(),
                    prompt_caching_enabled,
                    anthropic_features_enabled,
                    (openai_cache_key, openai_cache_retention),
                    self.reasoning_origin,
                );
                if !self.router_body_extensions {
                    api_request.reasoning_effort = None;
                }

                debug!(
                    model = %model,
                    messages = api_request.messages.len(),
                    tools = api_request.tools.as_ref().map_or(0, Vec::len),
                    body_cap_override = ?ctx.body_cap_override,
                    "Sending request to Anthropic"
                );

                let messages_count = api_request.messages.len();
                let response = self
                    .send_checked_with_cap(
                        request_ref,
                        &model,
                        &api_request,
                        messages_count,
                        ctx.body_cap_override,
                    )
                    .await?;
                let latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
                // Capture x-request-id before `.json()` consumes the
                // response body — otherwise the headers are gone by
                // the time we build the `ProviderTrace`. Mirrors the
                // streaming capture below; both paths feed into the
                // same `ProviderTrace.provider_request_id`.
                let provider_request_id = response
                    .headers()
                    .get("x-request-id")
                    .or_else(|| response.headers().get("request-id"))
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let plan_usage = crate::plan_usage::from_headers_now(response.headers());
                let api_response: ApiResponse = match response.json().await {
                    Ok(value) => value,
                    Err(e) => {
                        // A buffered body can terminate early after HTTP 200.
                        // No output has escaped yet, so retry its transport
                        // failure; a complete but invalid JSON body stays Parse.
                        let is_transport = is_retryable_transport_error(&e);
                        error!(error = %ErrorChain(&e), "Failed to read Anthropic response");
                        let err_str = ErrorChain(&e).to_string();
                        crate::console::anthropic_failure_block(
                            &crate::console::AnthropicFailureView {
                                status_code: Some(200),
                                status_text: "OK",
                                class: if is_transport { "transport" } else { "parse" },
                                elapsed_ms: latency_ms,
                                request_id: provider_request_id.as_deref(),
                                retry_after_s: None,
                                body_preview: Some(&err_str),
                                destination: "aura-network",
                            },
                        );
                        return Err(if is_transport {
                            classify_transport_error(&e, "Failed to read Anthropic response")
                        } else {
                            ApiError::Other(ReasonerError::Parse(format!(
                                "Failed to parse Anthropic response: {}",
                                ErrorChain(&e)
                            )))
                        });
                    }
                };
                let mut parsed = parse_complete_response(
                    api_response,
                    ctx.model_idx,
                    request_ref.model.as_ref(),
                    &model,
                    latency_ms,
                    provider_request_id,
                );
                parsed.trace.plan_usage = plan_usage;
                tag_response(&mut parsed, self.reasoning_origin);
                crate::console::emit_response_block(&parsed, latency_ms, 200, "OK");
                Ok(parsed)
            })
        })
        .await
    }

    async fn health_check(&self) -> bool {
        self.check_base_url_reachable().await
    }

    #[tracing::instrument(level = "debug", skip(self, request), fields(model = %request.model))]
    async fn complete_streaming(
        &self,
        request: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        let models = self.model_chain(request.model.as_ref());
        let request_ref = &request;

        run_model_chain_with_retries(&self.config, &models, |ctx, model| {
            Box::pin(async move {
                if !self.sse_for_all_models
                    && !Self::supports_anthropic_proxy_features(request_ref, &model)
                {
                    debug!(
                        model = %model,
                        "Router-backed fallback model does not support Anthropic SSE; buffering completion"
                    );
                    let mut buffered_request = request_ref.clone();
                    buffered_request.model = crate::ModelName::from(model.as_str());
                    let response = self
                        .complete(buffered_request)
                        .await
                        .map_err(ApiError::Other)?;
                    let shape = response_output_shape(&response);
                    debug!(
                        model = %model,
                        response_model = %response.model_used,
                        content_block_count = shape.content_block_count,
                        aggregate_text_bytes = shape.text_bytes,
                        thinking_bytes = shape.thinking_bytes,
                        tool_use_count = shape.tool_use_count,
                        stop_reason = ?response.stop_reason,
                        provider_request_id = ?response.trace.provider_request_id,
                        "Buffered proxy completion returned response shape"
                    );
                    return Ok(stream_from_response(response));
                }

                let prompt_caching_enabled =
                    self.prompt_caching_enabled_for_model(request_ref, &model);
                let anthropic_features_enabled =
                    self.anthropic_request_features_enabled(request_ref, &model);
                let openai_features_enabled =
                    Self::supports_openai_proxy_features(request_ref, &model);
                let system = build_request_system(
                    request_ref,
                    prompt_caching_enabled,
                    system_salt(request_ref, openai_features_enabled),
                );
                let thinking = anthropic_features_enabled
                    .then(|| resolve_thinking(request_ref, &model))
                    .flatten();
                let output_config = anthropic_features_enabled
                    .then(|| resolve_output_config(request_ref, &model))
                    .flatten();
                let (openai_cache_key, openai_cache_retention) = if openai_features_enabled {
                    (
                        // Already clamped to OpenAI's 64-char limit when the
                        // `ModelRequest` was built (see `ModelRequestBuilder::try_build`).
                        request_ref.prompt_cache_key.clone(),
                        request_ref
                            .prompt_cache_retention
                            .map(crate::types::PromptCacheRetention::as_wire),
                    )
                } else {
                    (None, None)
                };
                let api_request = StreamingApiRequest {
                    model: model.clone(),
                    system: system.clone(),
                    messages: convert_messages_to_api(
                        &request_ref.messages,
                        prompt_caching_enabled,
                        self.reasoning_origin,
                    ),
                    tools: if request_ref.tools.is_empty() {
                        None
                    } else {
                        Some(convert_tool_entries_to_api(
                            &request_ref.tools,
                            prompt_caching_enabled,
                            request_ref.cache_salt.as_deref(),
                        ))
                    },
                    tool_choice: if request_ref.tools.is_empty() {
                        None
                    } else {
                        tool_choice_for(
                            &request_ref.tool_choice,
                            request_ref.parallel_tool_use,
                            &model,
                        )
                    },
                    max_tokens: request_ref.max_tokens.get(),
                    temperature: if thinking.is_some() {
                        Some(1.0)
                    } else {
                        request_ref.temperature.map(f32::from)
                    },
                    stream: true,
                    thinking,
                    output_config,
                    reasoning_effort: request_ref
                        .thinking_effort
                        .and_then(|effort| effort.reasoning_effort_wire())
                        .filter(|_| self.router_body_extensions),
                    prompt_cache_key: openai_cache_key,
                    prompt_cache_retention: openai_cache_retention,
                };

                debug!(
                    model = %model,
                    messages = api_request.messages.len(),
                    tools = api_request.tools.as_ref().map_or(0, Vec::len),
                    body_cap_override = ?ctx.body_cap_override,
                    "Sending streaming request to Anthropic"
                );

                let messages_count = api_request.messages.len();
                let response = self
                    .send_checked_with_cap(
                        request_ref,
                        &model,
                        &api_request,
                        messages_count,
                        ctx.body_cap_override,
                    )
                    .await?;
                if ctx.model_idx > 0 {
                    info!(
                        primary = %request_ref.model,
                        fallback = %model,
                        "Streaming with fallback model"
                    );
                }
                // Capture x-request-id BEFORE `bytes_stream()` consumes
                // the response. Once the body is drained, the response
                // headers are gone, and a mid-stream SSE error would
                // otherwise surface with no correlatable id — see the
                // `diagnose-single-retry-llm-500` plan, F1. Fall back
                // to the non-standard `request-id` header for proxies
                // that rewrite the name.
                let provider_request_id = response
                    .headers()
                    .get("x-request-id")
                    .or_else(|| response.headers().get("request-id"))
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let plan_usage = crate::plan_usage::from_headers_now(response.headers());
                let byte_stream = response.bytes_stream();
                let sse_stream = SseStream::with_request_id(byte_stream, provider_request_id)
                    .with_plan_usage(plan_usage)
                    .with_idle_timeout(Duration::from_millis(self.config.timeout_ms));
                // The buffered path above is already tagged by `complete`.
                Ok(tag_stream(
                    Box::pin(sse_stream) as StreamEventStream,
                    self.reasoning_origin,
                ))
            })
        })
        .await
    }
}
