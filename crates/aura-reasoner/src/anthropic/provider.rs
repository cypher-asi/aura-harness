use super::api_types::{ApiRequest, ApiResponse, StreamingApiRequest};
use super::convert::{
    build_system_block, convert_messages_to_api, convert_response_to_aura, convert_tool_choice,
    convert_tools_to_api, resolve_output_config, resolve_thinking,
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

    fn anthropic_request_features_enabled(&self, request: &ModelRequest, model: &str) -> bool {
        match self.config.routing_mode {
            super::RoutingMode::Proxy => Self::supports_anthropic_proxy_features(request, model),
            super::RoutingMode::Direct => Self::model_looks_like_anthropic(model),
        }
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
        model: &str,
        json_body: &B,
    ) -> Result<reqwest::Response, ApiError> {
        let req_builder = self.build_request(request_ctx, model, json_body)?;

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
        model: &str,
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
        let proxy_prompt_caching_enabled =
            prompt_caching_enabled && Self::supports_anthropic_proxy_features(request_ctx, model);

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
    let header_retry_after = parse_retry_after_header(response.headers());
    // Pull any quota / request-id headers before consuming the response body so
    // 429/529 failures are easier to correlate with proxy-side logs.
    let request_id = response
        .headers()
        .get("x-request-id")
        .or_else(|| response.headers().get("request-id"))
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = response.text().await.unwrap_or_default();
    let body_preview = crate::truncate_body(&body, 200);
    error!(
        status = %status,
        body = %body_preview,
        retry_after_s = ?header_retry_after.map(|d| d.as_secs()),
        request_id = ?request_id,
        "Anthropic API error"
    );

    if super::is_cloudflare_html(&body) {
        return ApiError::CloudflareBlock(format!(
            "LLM proxy returned Cloudflare block ({status}) — service may be cold-starting"
        ));
    }

    match status_code {
        402 => ApiError::InsufficientCredits(format!("Anthropic API error: {status} - {body}")),
        429 | 529 => {
            let body_retry_after = parse_retry_after_from_body(&body);
            let retry_after = header_retry_after.or(body_retry_after);
            ApiError::Overloaded {
                message: format!("Anthropic API error: {status} - {body}"),
                retry_after,
            }
        }
        _ => ApiError::Other(ReasonerError::Api {
            status: status_code,
            message: format!("{status} - {body}"),
        }),
    }
}

/// Parse the HTTP `Retry-After` header. Supports both the seconds form
/// (e.g. `7`) and the HTTP-date form. Returns `None` when absent or
/// unparseable — callers fall back to the body hint or exp-backoff.
fn parse_retry_after_header(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let trimmed = raw.trim();
    if let Ok(secs) = trimmed.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    if let Ok(secs_f) = trimmed.parse::<f64>() {
        if secs_f.is_finite() && secs_f > 0.0 {
            return Some(Duration::from_secs_f64(secs_f));
        }
    }
    // HTTP-date form is not used by the aura-router proxy; skip it to
    // avoid pulling in an extra date-parsing dep.
    None
}

/// Parse a retry-after hint from a JSON body returned by the proxy. Recognised
/// shapes:
///
///   {"error":{"code":"RATE_LIMITED","message":"... Retry after 7 seconds."}}
///   {"error":{"retry_after":7, ...}}
///   {"retry_after":7, ...}
///
/// The harness's rate-limit proxy embeds the wait time in the `message` field,
/// so prose-parsing is required in addition to structured fields.
fn parse_retry_after_from_body(body: &str) -> Option<Duration> {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        let structured = json
            .get("retry_after")
            .or_else(|| json.get("error").and_then(|e| e.get("retry_after")))
            .and_then(|v| v.as_u64());
        if let Some(secs) = structured {
            return Some(Duration::from_secs(secs));
        }
    }
    parse_retry_after_prose(body)
}

/// Best-effort parse of `retry after N seconds?` (case-insensitive) from any
/// free-form text. This covers both the raw body and proxy messages embedded
/// inside JSON.
fn parse_retry_after_prose(text: &str) -> Option<Duration> {
    let lower = text.to_ascii_lowercase();
    let mut search_from = 0usize;
    while let Some(idx) = lower[search_from..].find("retry after") {
        let after = search_from + idx + "retry after".len();
        let rest = &lower[after..];
        let digits: String = rest
            .chars()
            .skip_while(|c| c.is_whitespace())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(secs) = digits.parse::<u64>() {
            return Some(Duration::from_secs(secs));
        }
        search_from = after;
    }
    None
}

fn build_api_request(
    request: &ModelRequest,
    model: &str,
    system: &serde_json::Value,
    prompt_caching_enabled: bool,
    anthropic_features_enabled: bool,
) -> ApiRequest {
    let thinking = anthropic_features_enabled
        .then(|| resolve_thinking(request, model))
        .flatten();
    let output_config = anthropic_features_enabled
        .then(|| resolve_output_config(request, model))
        .flatten();
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
        max_tokens: request.max_tokens.get(),
        temperature: if thinking.is_some() {
            Some(1.0)
        } else {
            request.temperature.map(f32::from)
        },
        thinking,
        output_config,
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

/// Outcome of `classify_retry_action`.
///
/// `Retry { sleep }` → sleep the given duration then attempt again with the
/// same model. `FallbackModel` → abandon this model, try the next in the
/// fallback chain. `Propagate` → give up, surface the underlying error.
#[derive(Debug)]
enum RetryAction {
    Retry { sleep: Duration },
    FallbackModel,
    Propagate,
}

/// Classify an `ApiError` into the next action for the retry loop.
///
/// For 429/529 we honour the upstream `Retry-After` hint (header or body) by
/// sleeping `max(retry_after, exponential_backoff)` plus a small jitter.
/// Previously this function used exponential backoff only (1s, 2s, 4s),
/// which — when the aura-router proxy reported `Retry after 7 seconds` —
/// burned every retry inside the rate-limit window and surfaced the 429 to
/// the user even though a single longer sleep would have unblocked the turn.
fn classify_retry_action(
    err: &ApiError,
    attempt: u32,
    max_retries: u32,
    model_idx: usize,
    model_count: usize,
    model: &str,
    last_err: &mut Option<ReasonerError>,
) -> RetryAction {
    match err {
        ApiError::CloudflareBlock(msg) if attempt < max_retries => {
            let sleep = exp_backoff_with_jitter(attempt);
            warn!(model = %model, attempt, backoff_ms = sleep.as_millis() as u64, "Cloudflare block, will retry");
            *last_err = Some(ReasonerError::Api {
                status: 403,
                message: msg.clone(),
            });
            RetryAction::Retry { sleep }
        }
        ApiError::Overloaded {
            message,
            retry_after,
        } if attempt < max_retries => {
            let sleep = sleep_for_overloaded(attempt, *retry_after);
            warn!(
                model = %model,
                attempt,
                backoff_ms = sleep.as_millis() as u64,
                retry_after_s = ?retry_after.map(|d| d.as_secs()),
                "API overloaded, will retry"
            );
            *last_err = Some(ReasonerError::RateLimited(
                super::format_rate_limited_message(message, *retry_after),
            ));
            RetryAction::Retry { sleep }
        }
        ApiError::Overloaded {
            message,
            retry_after,
        } if model_idx < model_count - 1 => {
            warn!(model = %model, "Retries exhausted, falling back to next model");
            *last_err = Some(ReasonerError::RateLimited(
                super::format_rate_limited_message(message, *retry_after),
            ));
            RetryAction::FallbackModel
        }
        _ => RetryAction::Propagate,
    }
}

/// Pure exponential backoff with small jitter for non-overloaded retries
/// (e.g. Cloudflare cold-starts). Caps at 30s.
fn exp_backoff_with_jitter(attempt: u32) -> Duration {
    let base_ms = 1000u64.saturating_mul(2u64.saturating_pow(attempt));
    let capped = base_ms.min(30_000);
    let jitter = jitter_ms(capped);
    Duration::from_millis(capped.saturating_add(jitter))
}

/// Compute the sleep before retrying an overloaded/429 error.
///
/// Returns `max(retry_after, exp_backoff) + jitter`. When the upstream tells
/// us to wait N seconds we always honour it (and then some), otherwise we
/// fall back to exponential backoff. Capped at 60s so a mis-reported
/// retry-after cannot wedge the loop indefinitely.
fn sleep_for_overloaded(attempt: u32, retry_after: Option<Duration>) -> Duration {
    let exp = exp_backoff_with_jitter(attempt);
    let chosen = match retry_after {
        // Pad by 500ms to clear the window edge.
        Some(hint) => exp.max(hint + Duration::from_millis(500)),
        None => exp,
    };
    chosen.min(Duration::from_secs(60))
}

/// Deterministic-ish low-amplitude jitter (0..=250ms) based on the current
/// instant. Using `Instant` avoids pulling in a `rand` dependency for a
/// harmless spread.
fn jitter_ms(base_ms: u64) -> u64 {
    let seed = Instant::now().elapsed().as_nanos() as u64;
    // Scale jitter to at most 25% of base, capped at 250ms.
    let max_jitter = (base_ms / 4).min(250);
    if max_jitter == 0 {
        0
    } else {
        seed % (max_jitter + 1)
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
        let models = self.model_chain(request.model.as_ref());
        let mut last_err: Option<ReasonerError> = None;

        for (model_idx, model) in models.iter().enumerate() {
            let prompt_caching_enabled = self.prompt_caching_enabled_for_model(&request, model);
            let anthropic_features_enabled =
                self.anthropic_request_features_enabled(&request, model);
            let system = build_system_block(&request.system, prompt_caching_enabled);
            let api_request = build_api_request(
                &request,
                model,
                &system,
                prompt_caching_enabled,
                anthropic_features_enabled,
            );

            debug!(
                model = %model,
                messages = api_request.messages.len(),
                tools = api_request.tools.as_ref().map_or(0, Vec::len),
                "Sending request to Anthropic"
            );

            let mut pending_sleep: Option<Duration> = None;
            for attempt in 0..=self.config.max_retries {
                if let Some(sleep) = pending_sleep.take() {
                    tokio::time::sleep(sleep).await;
                }

                match self.send_checked(&request, model, &api_request).await {
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
                            request.model.as_ref(),
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
                            RetryAction::Retry { sleep } => {
                                pending_sleep = Some(sleep);
                            }
                            RetryAction::FallbackModel => break,
                            RetryAction::Propagate => return Err(e.into()),
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
        let models = self.model_chain(request.model.as_ref());
        let mut last_err: Option<ReasonerError> = None;

        for (model_idx, model) in models.iter().enumerate() {
            if self.config.routing_mode == super::RoutingMode::Proxy
                && !Self::supports_anthropic_proxy_features(&request, model)
            {
                debug!(
                    model = %model,
                    "Proxy-backed fallback model does not support Anthropic SSE; buffering completion"
                );
                let mut buffered_request = request.clone();
                buffered_request.model = crate::ModelName::from(model.as_str());
                let response = self.complete(buffered_request).await?;
                return Ok(stream_from_response(response));
            }

            let prompt_caching_enabled = self.prompt_caching_enabled_for_model(&request, model);
            let anthropic_features_enabled =
                self.anthropic_request_features_enabled(&request, model);
            let system = build_system_block(&request.system, prompt_caching_enabled);
            let thinking = anthropic_features_enabled
                .then(|| resolve_thinking(&request, model))
                .flatten();
            let output_config = anthropic_features_enabled
                .then(|| resolve_output_config(&request, model))
                .flatten();
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
                max_tokens: request.max_tokens.get(),
                temperature: if thinking.is_some() {
                    Some(1.0)
                } else {
                    request.temperature.map(f32::from)
                },
                stream: true,
                thinking,
                output_config,
            };

            debug!(
                model = %model,
                messages = api_request.messages.len(),
                tools = api_request.tools.as_ref().map_or(0, Vec::len),
                "Sending streaming request to Anthropic"
            );

            let mut pending_sleep: Option<Duration> = None;
            for attempt in 0..=self.config.max_retries {
                if let Some(sleep) = pending_sleep.take() {
                    tokio::time::sleep(sleep).await;
                }

                match self.send_checked(&request, model, &api_request).await {
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
                            RetryAction::Retry { sleep } => {
                                pending_sleep = Some(sleep);
                            }
                            RetryAction::FallbackModel => break,
                            RetryAction::Propagate => return Err(e.into()),
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

#[cfg(test)]
mod retry_tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

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

    #[test]
    fn sleep_for_overloaded_waits_past_the_upstream_hint() {
        // Attempt 0 would otherwise sleep ~1s of exp backoff, but the upstream
        // told us 7s — the next attempt must land after the window.
        let sleep = sleep_for_overloaded(0, Some(Duration::from_secs(7)));
        assert!(
            sleep >= Duration::from_millis(7_500),
            "sleep ({:?}) must clear the 7s retry-after window",
            sleep
        );
        assert!(
            sleep <= Duration::from_secs(60),
            "sleep must be capped at 60s, got {:?}",
            sleep
        );
    }

    #[test]
    fn sleep_for_overloaded_caps_absurd_retry_after() {
        let sleep = sleep_for_overloaded(0, Some(Duration::from_secs(3600)));
        assert!(
            sleep <= Duration::from_secs(60),
            "sleep must be capped at 60s, got {:?}",
            sleep
        );
    }

    #[test]
    fn sleep_for_overloaded_falls_back_to_exp_backoff_without_hint() {
        // attempt=0 → base 1s + up to 250ms jitter
        let sleep = sleep_for_overloaded(0, None);
        assert!(sleep >= Duration::from_secs(1));
        assert!(sleep <= Duration::from_millis(1_250) + Duration::from_millis(50));
    }

    #[test]
    fn classify_retry_action_honours_retry_after_hint() {
        let err = ApiError::Overloaded {
            message: "429 rate limited".into(),
            retry_after: Some(Duration::from_secs(7)),
        };
        let mut last_err = None;
        let action = classify_retry_action(&err, 0, 2, 0, 1, "test-model", &mut last_err);
        match action {
            RetryAction::Retry { sleep } => {
                assert!(
                    sleep >= Duration::from_millis(7_500),
                    "retry sleep ({:?}) must clear the 7s upstream window",
                    sleep
                );
            }
            other => panic!("expected Retry, got {:?}", other),
        }
        match last_err {
            Some(ReasonerError::RateLimited(msg)) => {
                assert!(
                    msg.to_ascii_lowercase().contains("retry after"),
                    "last_err should surface the retry-after hint: {msg}"
                );
            }
            other => panic!("expected RateLimited, got {:?}", other),
        }
    }

    #[test]
    fn classify_retry_action_falls_back_after_exhausting_retries() {
        let err = ApiError::Overloaded {
            message: "429 rate limited".into(),
            retry_after: None,
        };
        let mut last_err = None;
        // attempt == max_retries → retries exhausted, fallback chain available
        let action = classify_retry_action(&err, 2, 2, 0, 2, "primary", &mut last_err);
        assert!(matches!(action, RetryAction::FallbackModel));
        assert!(matches!(last_err, Some(ReasonerError::RateLimited(_))));
    }

    #[test]
    fn classify_retry_action_propagates_when_no_fallback_available() {
        let err = ApiError::Overloaded {
            message: "429 rate limited".into(),
            retry_after: None,
        };
        let mut last_err = None;
        // attempt == max_retries AND model_idx == model_count - 1 → no fallback left
        let action = classify_retry_action(&err, 2, 2, 0, 1, "only", &mut last_err);
        assert!(matches!(action, RetryAction::Propagate));
    }

    #[test]
    fn classify_retry_action_other_errors_propagate() {
        let err = ApiError::Other(ReasonerError::Request("boom".into()));
        let mut last_err = None;
        let action = classify_retry_action(&err, 0, 2, 0, 1, "m", &mut last_err);
        assert!(matches!(action, RetryAction::Propagate));
    }
}
