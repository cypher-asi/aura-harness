use super::api_types::{ApiRequest, ApiResponse, StreamingApiRequest};
use super::convert::{
    build_system_block, convert_messages_to_api, convert_response_to_aura, convert_tool_choice,
    convert_tools_to_api, resolve_output_config, resolve_thinking,
};
use super::sse::SseStream;
use super::{AnthropicProvider, ApiError};

use crate::error::ReasonerError;
use crate::{
    emit_retry, response_output_shape, stream_from_response, ModelProvider, ModelRequest,
    ModelResponse, ProviderTrace, RetryInfo, StopReason, StreamEventStream, Usage,
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
            && Self::supports_anthropic_proxy_features(request, model)
    }

    fn anthropic_request_features_enabled(&self, request: &ModelRequest, model: &str) -> bool {
        Self::supports_anthropic_proxy_features(request, model)
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
        let token = request_ctx.auth_token.as_deref().ok_or_else(|| {
            ApiError::Other(ReasonerError::Internal(
                "router auth token missing".into(),
            ))
        })?;

        let mut req_builder = self
            .client
            .post(format!("{}/v1/messages", self.config.base_url))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .json(json_body);

        if self.config.prompt_caching_enabled
            && Self::supports_anthropic_proxy_features(request_ctx, model)
        {
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
        if let Some(ref family) = request_ctx.upstream_provider_family {
            let family = family.trim();
            if !family.is_empty() {
                req_builder = req_builder.header("X-Aura-Upstream-Provider-Family", family);
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
        if let Ok(dir) = std::env::var("AURA_DEBUG_CLOUDFLARE_DUMP_DIR") {
            let dir_path = std::path::PathBuf::from(&dir);
            if std::fs::create_dir_all(&dir_path).is_ok() {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                let file = dir_path.join(format!("cf-block-{ts}.html"));
                let header_dump = format!(
                    "<!-- aura-debug status={status} request_id={request_id:?} retry_after_s={:?} -->\n",
                    header_retry_after.map(|d| d.as_secs())
                );
                let _ = std::fs::write(&file, format!("{header_dump}{body}"));
                error!(
                    cloudflare_dump_path = %file.display(),
                    "Cloudflare HTML dumped for diagnosis"
                );
            }
        }
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
        // Axis 2: generic 5xx from the upstream LLM / proxy. Routed
        // through the retry path with bounded exponential backoff so a
        // single provider blip (`500 Internal server error`, `502 Bad
        // gateway`, `503 Service Unavailable` with a non-Cloudflare
        // body, `504 Gateway Timeout`) doesn't immediately surface as
        // a terminal failure to the dev loop. 501/505..=511 are left
        // as `Other` — those are configuration or protocol errors that
        // retrying will not fix.
        500 | 502 | 504 => ApiError::TransientServer {
            status: status_code,
            message: format!("Anthropic API error: {status} - {body}"),
        },
        // 503 hits the Cloudflare short-circuit above when the body
        // matches — anything else is a real upstream 503.
        503 => ApiError::TransientServer {
            status: status_code,
            message: format!("Anthropic API error: {status} - {body}"),
        },
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
    provider_request_id: Option<String>,
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
            message_id: Some(api_response.id),
            provider_request_id,
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
#[allow(clippy::too_many_arguments)]
fn classify_retry_action(
    err: &ApiError,
    attempt: u32,
    max_retries: u32,
    backoff_initial_ms: u64,
    backoff_cap_ms: u64,
    model_idx: usize,
    model_count: usize,
    model: &str,
    last_err: &mut Option<ReasonerError>,
) -> RetryAction {
    match err {
        ApiError::CloudflareBlock(msg) if attempt < max_retries => {
            let sleep = exp_backoff_with_jitter(attempt, backoff_initial_ms, backoff_cap_ms);
            // `Duration::as_millis` returns u128 but 30s backoff caps well below
            // u64::MAX; truncation cannot happen. `warn!` field value expressions
            // can't carry attributes directly, so bind first.
            #[allow(clippy::cast_possible_truncation)]
            let backoff_ms = sleep.as_millis() as u64;
            warn!(model = %model, attempt, backoff_ms, "Cloudflare block, will retry");
            *last_err = Some(ReasonerError::Transient {
                status: 403,
                message: msg.clone(),
                retry_after: None,
            });
            RetryAction::Retry { sleep }
        }
        ApiError::Overloaded {
            message,
            retry_after,
        } if attempt < max_retries => {
            let sleep =
                sleep_for_overloaded(attempt, *retry_after, backoff_initial_ms, backoff_cap_ms);
            // 60s cap on `sleep_for_overloaded` means u128 -> u64 is safe here.
            #[allow(clippy::cast_possible_truncation)]
            let backoff_ms = sleep.as_millis() as u64;
            warn!(
                model = %model,
                attempt,
                backoff_ms,
                retry_after_s = ?retry_after.map(|d| d.as_secs()),
                "API overloaded, will retry"
            );
            *last_err = Some(ReasonerError::RateLimited {
                message: super::format_rate_limited_message(message, *retry_after),
                retry_after: *retry_after,
            });
            RetryAction::Retry { sleep }
        }
        ApiError::Overloaded {
            message,
            retry_after,
        } if model_idx < model_count - 1 => {
            warn!(model = %model, "Retries exhausted, falling back to next model");
            *last_err = Some(ReasonerError::RateLimited {
                message: super::format_rate_limited_message(message, *retry_after),
                retry_after: *retry_after,
            });
            RetryAction::FallbackModel
        }
        // Axis 2: retry generic 5xx just like Cloudflare cold-starts,
        // using the same exponential-backoff-with-jitter schedule.
        // These resolve on the order of seconds on the provider side;
        // `exp_backoff_with_jitter` caps at 30s so we never wedge the
        // dev loop behind a single provider incident.
        ApiError::TransientServer { status, message } if attempt < max_retries => {
            let sleep = exp_backoff_with_jitter(attempt, backoff_initial_ms, backoff_cap_ms);
            #[allow(clippy::cast_possible_truncation)]
            let backoff_ms = sleep.as_millis() as u64;
            warn!(
                model = %model,
                attempt,
                status = *status,
                backoff_ms,
                "Upstream 5xx, will retry"
            );
            *last_err = Some(ReasonerError::Transient {
                status: *status,
                message: message.clone(),
                retry_after: None,
            });
            RetryAction::Retry { sleep }
        }
        // After retries are exhausted, try the fallback model rather
        // than surfacing the 5xx to the caller — the same escape hatch
        // we already give 429/529 overload errors.
        ApiError::TransientServer { status, message } if model_idx < model_count - 1 => {
            warn!(
                model = %model,
                status = *status,
                "5xx retries exhausted, falling back to next model"
            );
            *last_err = Some(ReasonerError::Transient {
                status: *status,
                message: message.clone(),
                retry_after: None,
            });
            RetryAction::FallbackModel
        }
        _ => RetryAction::Propagate,
    }
}

/// Classify an `ApiError` into the `reason` string we expose through
/// [`RetryInfo::reason`]. Keep the strings stable: the aura-harness
/// debug-event pipeline writes them verbatim into `retries.jsonl`.
fn retry_reason_for(err: &ApiError) -> &'static str {
    match err {
        ApiError::Overloaded { .. } => "rate_limited_429",
        ApiError::CloudflareBlock(_) => "transient_5xx",
        // Axis 2: distinct label so the dev loop can tell a real
        // upstream 5xx apart from Cloudflare cold-starts in
        // `retries.jsonl` (the heuristic reports bucket by reason).
        ApiError::TransientServer { .. } => "upstream_5xx",
        ApiError::InsufficientCredits(_) => "insufficient_credits",
        ApiError::Other(_) => "transient",
    }
}

/// Emit a `debug.retry` observation to the task-local observer (if
/// any). `attempt_that_failed` is the 0-based attempt counter of the
/// call that just failed; the 1-based "upcoming" attempt number is
/// `attempt_that_failed + 2`.
fn emit_retry_observation(err: &ApiError, sleep: Duration, attempt_that_failed: u32, model: &str) {
    let wait_ms = u64::try_from(sleep.as_millis()).unwrap_or(u64::MAX);
    let info = RetryInfo {
        reason: retry_reason_for(err).to_string(),
        attempt: attempt_that_failed.saturating_add(2),
        wait_ms,
        provider: "anthropic".to_string(),
        model: model.to_string(),
    };
    emit_retry(info);
}

/// Drive an attempt closure across the provider's model fallback chain
/// with the retry / backoff schedule set by [`super::AnthropicConfig`].
///
/// `attempt(model_idx, model)` performs one full request → response
/// round-trip for the given model and returns either `Ok(T)` (success,
/// returned immediately to the caller) or `Err(ApiError)` (consumed by
/// [`classify_retry_action`] to decide between sleeping + retrying,
/// dropping to the next model in the chain, or propagating). The
/// classification, exponential-backoff schedule, and `last_err`
/// surfacing logic stay in one place so the streaming and
/// non-streaming `ModelProvider` impls below differ only in the
/// per-attempt body — see the bullet on "Anthropic retry loops" in
/// the system-audit refactor plan.
///
/// Errors are surfaced as `ReasonerError` (the trait error type); the
/// classifier converts the underlying `ApiError` so callers don't have
/// to handle the internal variant.
type AttemptFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, ApiError>> + Send + 'a>>;

async fn run_model_chain_with_retries<'env, T, F>(
    config: &super::AnthropicConfig,
    models: &[String],
    mut attempt: F,
) -> Result<T, ReasonerError>
where
    F: FnMut(usize, String) -> AttemptFuture<'env, T> + 'env,
{
    let mut last_err: Option<ReasonerError> = None;

    'outer: for (model_idx, model) in models.iter().enumerate() {
        let mut pending_sleep: Option<Duration> = None;
        for try_n in 0..=config.max_retries {
            if let Some(sleep) = pending_sleep.take() {
                tokio::time::sleep(sleep).await;
            }

            match attempt(model_idx, model.clone()).await {
                Ok(value) => return Ok(value),
                Err(e) => match classify_retry_action(
                    &e,
                    try_n,
                    config.max_retries,
                    config.backoff_initial_ms,
                    config.backoff_cap_ms,
                    model_idx,
                    models.len(),
                    model,
                    &mut last_err,
                ) {
                    RetryAction::Retry { sleep } => {
                        emit_retry_observation(&e, sleep, try_n, model);
                        pending_sleep = Some(sleep);
                    }
                    RetryAction::FallbackModel => continue 'outer,
                    RetryAction::Propagate => return Err(e.into()),
                },
            }
        }
    }

    Err(last_err.unwrap_or_else(|| {
        ReasonerError::Internal("All models in fallback chain exhausted".into())
    }))
}

/// Pure exponential backoff with small jitter for non-overloaded retries
/// (e.g. Cloudflare cold-starts, per-tool-call streaming retries in
/// `aura_agent::agent_loop::streaming`).
///
/// The `initial_ms` and `cap_ms` parameters come from
/// [`super::AnthropicConfig::backoff_initial_ms`] /
/// [`super::AnthropicConfig::backoff_cap_ms`] (env-overridable via
/// `AURA_LLM_BACKOFF_INITIAL_MS` / `AURA_LLM_BACKOFF_CAP_MS`) so
/// operators can widen the window without rebuilding. `pub` because
/// the agent crate reuses this exact schedule for its per-tool-call
/// retry loop.
#[must_use]
pub fn exp_backoff_with_jitter(attempt: u32, initial_ms: u64, cap_ms: u64) -> Duration {
    let base_ms = initial_ms.saturating_mul(2u64.saturating_pow(attempt));
    let capped = base_ms.min(cap_ms);
    let jitter = jitter_ms(capped);
    Duration::from_millis(capped.saturating_add(jitter))
}

/// Compute the sleep before retrying an overloaded/429 error.
///
/// Returns `max(retry_after, exp_backoff) + jitter`. When the upstream tells
/// us to wait N seconds we always honour it (and then some), otherwise we
/// fall back to exponential backoff. Capped at 60s so a mis-reported
/// retry-after cannot wedge the loop indefinitely.
fn sleep_for_overloaded(
    attempt: u32,
    retry_after: Option<Duration>,
    backoff_initial_ms: u64,
    backoff_cap_ms: u64,
) -> Duration {
    let exp = exp_backoff_with_jitter(attempt, backoff_initial_ms, backoff_cap_ms);
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
    // Low-amplitude jitter only; we intentionally discard the high 64
    // bits of `as_nanos()` because we only need entropy, not precision.
    #[allow(clippy::cast_possible_truncation)]
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
        let request_ref = &request;

        run_model_chain_with_retries(&self.config, &models, |model_idx, model| {
            Box::pin(async move {
                let prompt_caching_enabled =
                    self.prompt_caching_enabled_for_model(request_ref, &model);
                let anthropic_features_enabled =
                    self.anthropic_request_features_enabled(request_ref, &model);
                let system = build_system_block(&request_ref.system, prompt_caching_enabled);
                let api_request = build_api_request(
                    request_ref,
                    &model,
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

                let response = self.send_checked(request_ref, &model, &api_request).await?;
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
                let api_response: ApiResponse = response.json().await.map_err(|e| {
                    error!(error = %e, "Failed to parse Anthropic response");
                    ApiError::Other(ReasonerError::Parse(format!(
                        "Failed to parse Anthropic response: {e}"
                    )))
                })?;
                Ok(parse_complete_response(
                    api_response,
                    model_idx,
                    request_ref.model.as_ref(),
                    &model,
                    latency_ms,
                    provider_request_id,
                ))
            })
        })
        .await
    }

    async fn health_check(&self) -> bool {
        self.check_base_url_reachable().await
    }

    #[tracing::instrument(skip(self, request), fields(model = %request.model))]
    async fn complete_streaming(
        &self,
        request: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        let models = self.model_chain(request.model.as_ref());
        let request_ref = &request;

        run_model_chain_with_retries(&self.config, &models, |model_idx, model| {
            Box::pin(async move {
                if !Self::supports_anthropic_proxy_features(request_ref, &model) {
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
                let system = build_system_block(&request_ref.system, prompt_caching_enabled);
                let thinking = anthropic_features_enabled
                    .then(|| resolve_thinking(request_ref, &model))
                    .flatten();
                let output_config = anthropic_features_enabled
                    .then(|| resolve_output_config(request_ref, &model))
                    .flatten();
                let api_request = StreamingApiRequest {
                    model: model.clone(),
                    system: system.clone(),
                    messages: convert_messages_to_api(
                        &request_ref.messages,
                        prompt_caching_enabled,
                    ),
                    tools: if request_ref.tools.is_empty() {
                        None
                    } else {
                        Some(convert_tools_to_api(
                            &request_ref.tools,
                            prompt_caching_enabled,
                        ))
                    },
                    tool_choice: convert_tool_choice(&request_ref.tool_choice),
                    max_tokens: request_ref.max_tokens.get(),
                    temperature: if thinking.is_some() {
                        Some(1.0)
                    } else {
                        request_ref.temperature.map(f32::from)
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

                let response = self.send_checked(request_ref, &model, &api_request).await?;
                if model_idx > 0 {
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
                let byte_stream = response.bytes_stream();
                let sse_stream = SseStream::with_request_id(byte_stream, provider_request_id);
                Ok(Box::pin(sse_stream) as StreamEventStream)
            })
        })
        .await
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
        let sleep = sleep_for_overloaded(0, Some(Duration::from_secs(7)), 1000, 30_000);
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
        let sleep = sleep_for_overloaded(0, Some(Duration::from_secs(3600)), 1000, 30_000);
        assert!(
            sleep <= Duration::from_secs(60),
            "sleep must be capped at 60s, got {:?}",
            sleep
        );
    }

    #[test]
    fn sleep_for_overloaded_falls_back_to_exp_backoff_without_hint() {
        // attempt=0 → base 1s + up to 250ms jitter
        let sleep = sleep_for_overloaded(0, None, 1000, 30_000);
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
        let action =
            classify_retry_action(&err, 0, 2, 1000, 30_000, 0, 1, "test-model", &mut last_err);
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
            Some(ReasonerError::RateLimited {
                ref message,
                retry_after,
            }) => {
                assert!(
                    message.to_ascii_lowercase().contains("retry after"),
                    "last_err should surface the retry-after hint: {message}"
                );
                assert_eq!(
                    retry_after,
                    Some(Duration::from_secs(7)),
                    "structured retry_after should match the upstream hint"
                );
            }
            ref other => panic!("expected RateLimited, got {:?}", other),
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
        let action =
            classify_retry_action(&err, 2, 2, 1000, 30_000, 0, 2, "primary", &mut last_err);
        assert!(matches!(action, RetryAction::FallbackModel));
        assert!(matches!(last_err, Some(ReasonerError::RateLimited { .. })));
    }

    #[test]
    fn classify_retry_action_propagates_when_no_fallback_available() {
        let err = ApiError::Overloaded {
            message: "429 rate limited".into(),
            retry_after: None,
        };
        let mut last_err = None;
        // attempt == max_retries AND model_idx == model_count - 1 → no fallback left
        let action = classify_retry_action(&err, 2, 2, 1000, 30_000, 0, 1, "only", &mut last_err);
        assert!(matches!(action, RetryAction::Propagate));
    }

    #[test]
    fn classify_retry_action_other_errors_propagate() {
        let err = ApiError::Other(ReasonerError::Request("boom".into()));
        let mut last_err = None;
        let action = classify_retry_action(&err, 0, 2, 1000, 30_000, 0, 1, "m", &mut last_err);
        assert!(matches!(action, RetryAction::Propagate));
    }

    // ---------- Axis 2 coverage ----------

    #[test]
    fn classify_retry_action_retries_transient_5xx_with_exp_backoff() {
        let err = ApiError::TransientServer {
            status: 500,
            message: "Anthropic API error: 500 Internal Server Error - body".into(),
        };
        let mut last_err = None;
        let action =
            classify_retry_action(&err, 0, 2, 1000, 30_000, 0, 1, "primary", &mut last_err);
        match action {
            RetryAction::Retry { sleep } => {
                // `exp_backoff_with_jitter(0)` → base 1s + up to 250ms jitter.
                assert!(
                    sleep >= Duration::from_secs(1),
                    "first-attempt 5xx backoff must be >= 1s, got {sleep:?}"
                );
                assert!(
                    sleep <= Duration::from_millis(1_300),
                    "first-attempt 5xx backoff must stay under the jitter cap, got {sleep:?}"
                );
            }
            other => panic!("expected Retry on 5xx, got {other:?}"),
        }
        match last_err {
            Some(ReasonerError::Transient { status, .. }) => assert_eq!(status, 500),
            other => panic!("expected ReasonerError::Transient last_err, got {other:?}"),
        }
    }

    #[test]
    fn classify_retry_action_falls_back_when_5xx_retries_exhausted() {
        let err = ApiError::TransientServer {
            status: 502,
            message: "Anthropic API error: 502 Bad Gateway - body".into(),
        };
        let mut last_err = None;
        let action =
            classify_retry_action(&err, 2, 2, 1000, 30_000, 0, 2, "primary", &mut last_err);
        assert!(
            matches!(action, RetryAction::FallbackModel),
            "expected FallbackModel after 5xx retries are used up"
        );
        match last_err {
            Some(ReasonerError::Transient { status, .. }) => assert_eq!(status, 502),
            other => panic!("expected ReasonerError::Transient last_err, got {other:?}"),
        }
    }

    #[test]
    fn classify_retry_action_propagates_5xx_when_no_fallback_available() {
        let err = ApiError::TransientServer {
            status: 504,
            message: "Anthropic API error: 504 Gateway Timeout - body".into(),
        };
        let mut last_err = None;
        let action = classify_retry_action(&err, 2, 2, 1000, 30_000, 0, 1, "only", &mut last_err);
        assert!(
            matches!(action, RetryAction::Propagate),
            "no fallback model → 5xx must propagate so the dev loop can retry"
        );
    }

    #[test]
    fn retry_reason_for_labels_transient_5xx_distinctly() {
        // `upstream_5xx` must be distinct from the Cloudflare-specific
        // `transient_5xx` bucket so run heuristics can separate
        // provider-internal outages from cold-start cloudflare
        // blocks in retry histograms.
        let err = ApiError::TransientServer {
            status: 503,
            message: "Anthropic API error: 503 - body".into(),
        };
        assert_eq!(retry_reason_for(&err), "upstream_5xx");
        assert_eq!(
            retry_reason_for(&ApiError::CloudflareBlock("cf".into())),
            "transient_5xx"
        );
    }
}
