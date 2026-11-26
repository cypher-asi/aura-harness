use super::body_cap::fit_body_under_cap;
use super::diagnostics::{
    dump_request_body_if_enabled, format_thinking_label, request_headers_present,
    strip_tool_choice_braces, summarize_anthropic_request,
};
use super::errors::{classify_api_error, classify_transport_error};
use super::throttle::throttle_outbound_request;
use super::waf::{defang_waf_command_patterns, serialize_request_body};
use super::{millis_as_u64, RequestRoutingContext};
use crate::anthropic::convert::{
    extended_cache_ttl, request_uses_computer_tool, COMPUTER_USE_BETA, EXTENDED_CACHE_TTL_BETA,
};
use crate::anthropic::{AnthropicProvider, ApiError};
use crate::error::ReasonerError;
use crate::{ModelContentProfile, ModelRequest};
use serde::Serialize;
use std::time::Duration;
use tracing::{debug, error, warn};

impl AnthropicProvider {
    pub(super) fn model_looks_like_anthropic(model: &str) -> bool {
        let model = crate::anthropic::convert::capability_model(model).to_ascii_lowercase();
        model.starts_with("claude") || model.starts_with("aura-claude")
    }

    pub(super) fn supports_anthropic_proxy_features(request: &ModelRequest, model: &str) -> bool {
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

    fn model_looks_like_openai(model: &str) -> bool {
        let model = model.trim().to_ascii_lowercase();
        model.starts_with("gpt-")
            || model.starts_with("aura-gpt-")
            || model.starts_with("o1-")
            || model.starts_with("o3-")
            || model.starts_with("o4-")
            || model == "gpt"
    }

    pub(super) fn supports_openai_proxy_features(request: &ModelRequest, model: &str) -> bool {
        if let Some(family) = request
            .upstream_provider_family
            .as_deref()
            .map(str::trim)
            .filter(|family| !family.is_empty())
        {
            return family.eq_ignore_ascii_case("openai");
        }
        Self::model_looks_like_openai(model)
    }

    pub(super) fn prompt_caching_enabled_for_model(
        &self,
        request: &ModelRequest,
        model: &str,
    ) -> bool {
        self.config.prompt_caching_enabled
            && Self::supports_anthropic_proxy_features(request, model)
    }

    pub(super) fn anthropic_request_features_enabled(
        &self,
        request: &ModelRequest,
        model: &str,
    ) -> bool {
        Self::supports_anthropic_proxy_features(request, model)
    }

    pub(super) async fn check_base_url_reachable(&self) -> bool {
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
    ///
    /// Serializes `json_body` exactly once into a `Vec<u8>` so we can:
    ///   1. emit a single `body_bytes` info-log line per outbound
    ///      request (Phase-0 hypothesis test for the Cloudflare 403s),
    ///   2. apply the optional `AURA_LLM_EMERGENCY_BODY_CAP_BYTES`
    ///      truncation in-place before the bytes ever leave the
    ///      process.
    ///
    /// `messages_count` is taken from the typed request so we don't
    /// have to re-parse the JSON; it is only used for the diagnostic
    /// log line.
    /// Send an HTTP request to the Anthropic API, classifying the
    /// response. The `body_cap_override` parameter lets the outer
    /// retry loop tighten the effective body cap for the current
    /// attempt; it is applied *only* when smaller than the configured
    /// `emergency_body_cap_bytes` (otherwise it would loosen the cap
    /// instead of tightening it on a Cloudflare retry, which is the
    /// exact regression we are guarding against).
    pub(super) async fn send_checked_with_cap<B: Serialize + Sync>(
        &self,
        request_ctx: &ModelRequest,
        model: &str,
        json_body: &B,
        messages_count: usize,
        body_cap_override: Option<usize>,
    ) -> Result<reqwest::Response, ApiError> {
        let content_profile = ModelContentProfile::from_request(request_ctx)
            .validate()
            .map_err(|violation| {
                ApiError::Other(ReasonerError::ModelRequestContractViolation(violation))
            })?;
        let body_bytes = serialize_request_body(json_body).map_err(|e| {
            ApiError::Other(ReasonerError::Internal(format!(
                "serialize Anthropic request body: {e}"
            )))
        })?;

        let effective_cap = self.effective_body_cap(body_cap_override);
        let capped_bytes = self.apply_body_cap(model, body_bytes, effective_cap);
        // WAF defang runs AFTER the cap so it sees the final wire bytes
        // (any patterns introduced by the cap's truncation marker pass
        // through this same step). Pre-cap defanging would risk silently
        // shifting byte offsets that the cap relies on.
        let final_bytes = defang_waf_command_patterns(capped_bytes);
        let wire_body_bytes = final_bytes.len();
        let request_summary = summarize_anthropic_request(&final_bytes);
        let debug_request_dump_path =
            dump_request_body_if_enabled(model, &request_summary.body_hash, &final_bytes);
        let routing_context = RequestRoutingContext::from_request(request_ctx);
        let prompt_caching_header_enabled = self.config.prompt_caching_enabled
            && Self::supports_anthropic_proxy_features(request_ctx, model);
        let headers_present_str =
            request_headers_present(request_ctx, prompt_caching_header_enabled);
        let request_kind_label = format!("{:?}", content_profile.kind);
        let tool_choice_label = request_summary
            .tool_choice
            .as_deref()
            .map_or_else(|| "n/a".to_string(), strip_tool_choice_braces);
        let thinking_label = format_thinking_label(
            request_summary.has_thinking,
            request_ctx.thinking_effort,
            request_summary.thinking_type.as_deref(),
            request_summary.thinking_budget_tokens,
        );

        // Visual block for human-scannable transcripts. The forensic
        // field-by-field log is preserved at `debug!` below so
        // operators can still grep / pivot on individual fields when
        // chasing WAF / cap regressions.
        let destination_host = crate::console::extract_host(&self.config.base_url);
        crate::console::anthropic_request_block(&crate::console::AnthropicRequestView {
            model,
            kind: &request_kind_label,
            body_bytes: wire_body_bytes,
            messages_count,
            tools_count: request_summary.tools_count,
            tool_choice: &tool_choice_label,
            tool_names: &request_summary.tool_names,
            thinking_label: &thinking_label,
            system_bytes: request_summary.system_bytes,
            last_user_bytes: request_summary.last_user_text_bytes,
            last_user_hash: request_summary.last_user_text_hash.as_deref(),
            headers_present: &headers_present_str,
            request_hash: &request_summary.body_hash,
            destination: "aura-network",
            destination_host,
        });

        debug!(
            model = %model,
            body_bytes = wire_body_bytes,
            messages_count,
            emergency_body_cap_bytes = self.config.emergency_body_cap_bytes,
            effective_body_cap_bytes = effective_cap,
            body_cap_override_bytes = ?body_cap_override,
            min_request_interval_ms = self.config.min_request_interval_ms,
            request_body_hash = %request_summary.body_hash,
            top_level_keys = %request_summary.top_level_keys,
            stream = request_summary.stream,
            system_bytes = request_summary.system_bytes,
            messages_text_bytes = request_summary.messages_text_bytes,
            last_user_text_bytes = request_summary.last_user_text_bytes,
            last_user_text_hash = ?request_summary.last_user_text_hash,
            tools_count = request_summary.tools_count,
            tool_names = %request_summary.tool_names,
            tool_choice = ?request_summary.tool_choice,
            request_kind = ?content_profile.kind,
            request_contract_verdict = ?content_profile.verdict,
            content_signature = %content_profile.content_signature,
            thinking = request_summary.has_thinking,
            output_config = request_summary.has_output_config,
            headers_present = %headers_present_str,
            aura_project_id = routing_context.project_label(),
            aura_agent_id = routing_context.agent_label(),
            aura_org_id = routing_context.org_label(),
            aura_session_id = routing_context.session_label(),
            upstream_provider_family = request_ctx
                .upstream_provider_family
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("missing"),
            debug_request_dump_path = ?debug_request_dump_path,
            "Anthropic /v1/messages request"
        );

        let req_builder =
            self.build_request(request_ctx, model, final_bytes, request_summary.stream)?;
        throttle_outbound_request(self.config.min_request_interval_ms, model).await;

        let send_started_at = std::time::Instant::now();

        // Bound connection/header stalls without expiring a healthy SSE body.
        let send_result = tokio::time::timeout(
            Duration::from_millis(self.config.timeout_ms),
            req_builder.send(),
        )
        .await
        .map_err(|_| ApiError::Transport(ReasonerError::Timeout))?;
        let response = match send_result {
            Ok(resp) => resp,
            Err(e) => {
                let elapsed_ms = millis_as_u64(send_started_at.elapsed().as_millis());
                error!(error = %e, "Model provider API request failed");
                let is_timeout = e.is_timeout();
                let err_str = e.to_string();
                let class = if is_timeout { "timeout" } else { "transport" };
                let status_text = if is_timeout {
                    "request timed out"
                } else {
                    "transport failed"
                };
                crate::console::anthropic_failure_block(&crate::console::AnthropicFailureView {
                    status_code: None,
                    status_text,
                    class,
                    elapsed_ms,
                    request_id: None,
                    retry_after_s: None,
                    body_preview: Some(&err_str),
                    destination: "aura-network",
                });
                return Err(classify_transport_error(
                    &e,
                    "Model provider API request failed",
                ));
            }
        };

        let elapsed_ms = millis_as_u64(send_started_at.elapsed().as_millis());

        if !response.status().is_success() {
            // Error bodies are buffered, even for streaming requests. They
            // still need a deadline when the SSE client has no total timeout.
            let (err, meta) = tokio::time::timeout(
                Duration::from_millis(self.config.timeout_ms),
                classify_api_error(
                    response,
                    RequestRoutingContext::from_request(request_ctx),
                    Some(&content_profile),
                    wire_body_bytes,
                ),
            )
            .await
            .map_err(|_| ApiError::Transport(ReasonerError::Timeout))?;
            crate::console::anthropic_failure_block(&crate::console::AnthropicFailureView {
                status_code: Some(meta.status_code),
                status_text: &meta.status_text,
                class: meta.class,
                elapsed_ms,
                request_id: meta.request_id.as_deref(),
                retry_after_s: meta.retry_after_s,
                body_preview: Some(&meta.body_preview),
                destination: "aura-network",
            });
            return Err(err);
        }

        Ok(response)
    }

    /// Resolve the effective body cap for one outbound attempt.
    ///
    /// When the retry loop has narrowed the cap on a 403 retry, the
    /// tightened value wins — but only if it is strictly smaller than
    /// the configured cap. The configured value remains the ceiling
    /// in all other cases. A configured cap of `0` (disabled) stays
    /// disabled regardless of override, because the operator has
    /// explicitly opted out of proactive trimming.
    pub(super) fn effective_body_cap(&self, body_cap_override: Option<usize>) -> usize {
        let configured = self.config.emergency_body_cap_bytes;
        if configured == 0 {
            return 0;
        }
        match body_cap_override {
            Some(override_cap) if override_cap < configured => override_cap,
            _ => configured,
        }
    }

    /// Apply the configured body cap (or per-attempt override) to a
    /// freshly serialised `/v1/messages` body. Returns a `Vec<u8>` that
    /// is **always** at or below `cap` (modulo the truncation marker
    /// budget) — the worst-case fallback collapses the conversation
    /// down to a single placeholder user message, so this function
    /// never lets an oversized request reach the wire.
    ///
    /// The historical name was `maybe_apply_emergency_body_cap`; we
    /// keep the env var name for backwards compatibility but the
    /// behaviour is no longer a "diagnostic" — proactive request
    /// budgeting is now part of the harness's reliability contract.
    pub(super) fn apply_body_cap(&self, model: &str, body_bytes: Vec<u8>, cap: usize) -> Vec<u8> {
        if cap == 0 || body_bytes.len() <= cap {
            return body_bytes;
        }

        let original_len = body_bytes.len();
        let (capped, dropped_messages, mode) = fit_body_under_cap(&body_bytes, cap);
        warn!(
            model = %model,
            original_bytes = original_len,
            capped_bytes = capped.len(),
            cap_bytes = cap,
            dropped_messages,
            fit_mode = %mode,
            "Request body exceeded `AURA_LLM_EMERGENCY_BODY_CAP_BYTES`; \
             trimmed in place to stay under the proactive Cloudflare safety budget"
        );
        capped
    }

    // The function is infallible today but kept on a `Result` shape
    // because every caller threads it through `?`; converting to a
    // plain return would force the callers to drop or re-wrap the
    // result and the current shape leaves room to surface
    // header/body validation errors without a downstream churn.
    #[allow(clippy::unnecessary_wraps)]
    fn build_request(
        &self,
        request_ctx: &ModelRequest,
        model: &str,
        body_bytes: Vec<u8>,
        streaming: bool,
    ) -> Result<reqwest::RequestBuilder, ApiError> {
        let token = request_ctx.auth_token.as_deref();

        let client = if streaming {
            &self.streaming_client
        } else {
            &self.client
        };
        let mut req_builder = client
            .post(format!("{}/v1/messages", self.config.base_url))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .body(body_bytes);

        // Only send Authorization header when a token is present.
        // Public-guest sessions have no token — the router accepts
        // unauthenticated requests and assigns user_id "public-guest".
        if let Some(token) = token {
            req_builder = req_builder.header("authorization", format!("Bearer {token}"));
        }

        // Assemble the `anthropic-beta` header. Prompt caching keeps its
        // exact prior token, and computer-use appends a second token only
        // when the run carries the `computer` tool — so non-computer-use
        // requests stay byte-identical.
        let mut beta_tokens: Vec<&str> = Vec::new();
        if self.config.prompt_caching_enabled
            && Self::supports_anthropic_proxy_features(request_ctx, model)
        {
            beta_tokens.push("prompt-caching-2024-07-31");
            if extended_cache_ttl() {
                beta_tokens.push(EXTENDED_CACHE_TTL_BETA);
            }
        }
        if request_uses_computer_tool(&request_ctx.tools) {
            beta_tokens.push(COMPUTER_USE_BETA);
        }
        for (name, value) in &request_ctx.extra_headers {
            if name.eq_ignore_ascii_case("anthropic-beta") {
                beta_tokens.extend(value.split(',').map(str::trim).filter(|v| !v.is_empty()));
            } else {
                req_builder = req_builder.header(name.as_str(), value.as_str());
            }
        }
        if !beta_tokens.is_empty() {
            req_builder = req_builder.header("anthropic-beta", beta_tokens.join(","));
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

        if Self::supports_openai_proxy_features(request_ctx, model) {
            if let Some(ref key) = request_ctx.prompt_cache_key {
                // Already clamped to OpenAI's 64-char limit when the
                // `ModelRequest` was built; the router maps this header
                // onto OpenAI's `prompt_cache_key`.
                req_builder = req_builder.header("X-Aura-Prompt-Cache-Key", key);
            }
            if let Some(retention) = request_ctx.prompt_cache_retention {
                req_builder =
                    req_builder.header("X-Aura-Prompt-Cache-Retention", retention.as_wire());
            }
        }

        Ok(req_builder)
    }
}
