use super::api_types::{ApiRequest, ApiResponse, StreamingApiRequest};
use super::convert::{
    build_system_block, convert_messages_to_api, convert_response_to_aura, convert_tool_choice,
    convert_tools_to_api, resolve_thinking,
};
use super::sse::SseStream;
use super::{AnthropicProvider, ApiError};

use crate::error::ReasonerError;
use crate::{
    ModelProvider, ModelRequest, ModelResponse, ProviderTrace, StopReason, StreamEventStream, Usage,
};
use async_trait::async_trait;
use serde::Serialize;
use std::time::Instant;
use tracing::{debug, error, info, warn};

impl AnthropicProvider {
    /// Send an HTTP request to the Anthropic API and classify the response.
    ///
    /// Returns the raw `reqwest::Response` on success, or an [`ApiError`]
    /// that the retry loop can pattern-match on.
    pub(super) async fn send_checked<B: Serialize + Sync>(
        &self,
        request_ctx: &ModelRequest,
        json_body: &B,
    ) -> Result<reqwest::Response, ApiError> {
        use super::config::RoutingMode;

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

            if super::is_cloudflare_html(&body) {
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

    #[tracing::instrument(skip(self, request), fields(model = %request.model))]
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

    async fn health_check(&self) -> bool {
        true
    }

    #[tracing::instrument(skip(self, request), fields(model = %request.model))]
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
