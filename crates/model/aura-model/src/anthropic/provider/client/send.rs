//! The outbound `/v1/messages` send path: serialize, cap, log, send
//! and classify the response of one attempt.

use crate::anthropic::provider::diagnostics::{
    dump_request_body_if_enabled, format_thinking_label, request_headers_present,
    strip_tool_choice_braces, summarize_anthropic_request, RequestDiagnosticsSummary,
};
use crate::anthropic::provider::errors::{classify_api_error, classify_transport_error};
use crate::anthropic::provider::throttle::throttle_outbound_request;
use crate::anthropic::provider::waf::{defang_waf_command_patterns, serialize_request_body};
use crate::anthropic::provider::{millis_as_u64, RequestRoutingContext};
use crate::anthropic::{AnthropicProvider, ApiError};
use crate::error::ReasonerError;
use crate::{ModelContentProfile, ModelRequest};
use serde::Serialize;
use std::time::{Duration, Instant};
use tracing::{debug, error};

/// What the request logs and the failure classifier need to know
/// about one outbound attempt once its final wire bytes are fixed.
struct OutboundRequest<'a> {
    request_ctx: &'a ModelRequest,
    model: &'a str,
    content_profile: &'a ModelContentProfile,
    summary: &'a RequestDiagnosticsSummary,
    wire_body_bytes: usize,
    messages_count: usize,
    effective_cap: usize,
    body_cap_override: Option<usize>,
    debug_request_dump_path: Option<String>,
    headers_present: String,
}

impl AnthropicProvider {
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
    pub(in crate::anthropic::provider) async fn send_checked_with_cap<B: Serialize + Sync>(
        &self,
        request_ctx: &ModelRequest,
        model: &str,
        json_body: &B,
        messages_count: usize,
        body_cap_override: Option<usize>,
    ) -> Result<reqwest::Response, ApiError> {
        let content_profile = validated_content_profile(request_ctx)?;
        let body_bytes = serialize_body(json_body)?;

        let effective_cap = self.effective_body_cap(body_cap_override);
        let capped_bytes = self.apply_body_cap(model, body_bytes, effective_cap);
        // WAF defang runs AFTER the cap so it sees the final wire bytes
        // (any patterns introduced by the cap's truncation marker pass
        // through this same step). Pre-cap defanging would risk silently
        // shifting byte offsets that the cap relies on.
        let final_bytes = defang_waf_command_patterns(capped_bytes);
        let request_summary = summarize_anthropic_request(&final_bytes);
        let outbound = OutboundRequest {
            request_ctx,
            model,
            content_profile: &content_profile,
            summary: &request_summary,
            wire_body_bytes: final_bytes.len(),
            messages_count,
            effective_cap,
            body_cap_override,
            debug_request_dump_path: dump_request_body_if_enabled(
                model,
                &request_summary.body_hash,
                &final_bytes,
            ),
            headers_present: request_headers_present(
                request_ctx,
                self.config.prompt_caching_enabled
                    && Self::supports_anthropic_proxy_features(request_ctx, model),
            ),
        };
        self.log_request_block(&outbound);
        self.log_request_fields(&outbound);

        let req_builder =
            self.build_request(request_ctx, model, final_bytes, request_summary.stream)?;
        throttle_outbound_request(self.config.min_request_interval_ms, model).await;

        let send_started_at = Instant::now();
        let response = self
            .send_within_timeout(req_builder, send_started_at)
            .await?;
        let elapsed_ms = millis_as_u64(send_started_at.elapsed().as_millis());

        if !response.status().is_success() {
            return Err(self
                .classify_failed_response(response, &outbound, elapsed_ms)
                .await);
        }

        Ok(response)
    }

    /// Visual block for human-scannable transcripts. The forensic
    /// field-by-field log is preserved at `debug!` in
    /// [`Self::log_request_fields`] so operators can still grep / pivot
    /// on individual fields when chasing WAF / cap regressions.
    fn log_request_block(&self, outbound: &OutboundRequest<'_>) {
        let summary = outbound.summary;
        let request_kind_label = format!("{:?}", outbound.content_profile.kind);
        let tool_choice_label = summary
            .tool_choice
            .as_deref()
            .map_or_else(|| "n/a".to_string(), strip_tool_choice_braces);
        let thinking_label = format_thinking_label(
            summary.has_thinking,
            outbound.request_ctx.thinking_effort,
            summary.thinking_type.as_deref(),
            summary.thinking_budget_tokens,
        );

        let destination_host = crate::console::extract_host(&self.config.base_url);
        crate::console::anthropic_request_block(&crate::console::AnthropicRequestView {
            model: outbound.model,
            kind: &request_kind_label,
            body_bytes: outbound.wire_body_bytes,
            messages_count: outbound.messages_count,
            tools_count: summary.tools_count,
            tool_choice: &tool_choice_label,
            tool_names: &summary.tool_names,
            thinking_label: &thinking_label,
            system_bytes: summary.system_bytes,
            last_user_bytes: summary.last_user_text_bytes,
            last_user_hash: summary.last_user_text_hash.as_deref(),
            headers_present: &outbound.headers_present,
            request_hash: &summary.body_hash,
            destination: "aura-network",
            destination_host,
        });
    }

    fn log_request_fields(&self, outbound: &OutboundRequest<'_>) {
        let summary = outbound.summary;
        let content_profile = outbound.content_profile;
        let routing_context = RequestRoutingContext::from_request(outbound.request_ctx);
        debug!(
            model = %outbound.model,
            body_bytes = outbound.wire_body_bytes,
            messages_count = outbound.messages_count,
            emergency_body_cap_bytes = self.config.emergency_body_cap_bytes,
            effective_body_cap_bytes = outbound.effective_cap,
            body_cap_override_bytes = ?outbound.body_cap_override,
            min_request_interval_ms = self.config.min_request_interval_ms,
            request_body_hash = %summary.body_hash,
            top_level_keys = %summary.top_level_keys,
            stream = summary.stream,
            system_bytes = summary.system_bytes,
            messages_text_bytes = summary.messages_text_bytes,
            last_user_text_bytes = summary.last_user_text_bytes,
            last_user_text_hash = ?summary.last_user_text_hash,
            tools_count = summary.tools_count,
            tool_names = %summary.tool_names,
            tool_choice = ?summary.tool_choice,
            request_kind = ?content_profile.kind,
            request_contract_verdict = ?content_profile.verdict,
            content_signature = %content_profile.content_signature,
            thinking = summary.has_thinking,
            output_config = summary.has_output_config,
            headers_present = %outbound.headers_present,
            aura_project_id = routing_context.project_label(),
            aura_agent_id = routing_context.agent_label(),
            aura_org_id = routing_context.org_label(),
            aura_session_id = routing_context.session_label(),
            upstream_provider_family = outbound
                .request_ctx
                .upstream_provider_family
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("missing"),
            debug_request_dump_path = ?outbound.debug_request_dump_path,
            "Anthropic /v1/messages request"
        );
    }

    async fn send_within_timeout(
        &self,
        req_builder: reqwest::RequestBuilder,
        send_started_at: Instant,
    ) -> Result<reqwest::Response, ApiError> {
        // Bound connection/header stalls without expiring a healthy SSE body.
        let send_result = tokio::time::timeout(
            Duration::from_millis(self.config.timeout_ms),
            req_builder.send(),
        )
        .await
        .map_err(|_| ApiError::Transport(ReasonerError::Timeout))?;
        send_result.map_err(|e| report_transport_failure(&e, send_started_at))
    }

    async fn classify_failed_response(
        &self,
        response: reqwest::Response,
        outbound: &OutboundRequest<'_>,
        elapsed_ms: u64,
    ) -> ApiError {
        // Error bodies are buffered, even for streaming requests. They
        // still need a deadline when the SSE client has no total timeout.
        let classified = tokio::time::timeout(
            Duration::from_millis(self.config.timeout_ms),
            classify_api_error(
                response,
                RequestRoutingContext::from_request(outbound.request_ctx),
                Some(outbound.content_profile),
                outbound.wire_body_bytes,
            ),
        )
        .await;
        let Ok((err, meta)) = classified else {
            return ApiError::Transport(ReasonerError::Timeout);
        };
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
        err
    }
}

fn validated_content_profile(request_ctx: &ModelRequest) -> Result<ModelContentProfile, ApiError> {
    ModelContentProfile::from_request(request_ctx)
        .validate()
        .map_err(|violation| {
            ApiError::Other(ReasonerError::ModelRequestContractViolation(violation))
        })
}

fn serialize_body<B: Serialize>(json_body: &B) -> Result<Vec<u8>, ApiError> {
    serialize_request_body(json_body).map_err(|e| {
        ApiError::Other(ReasonerError::Internal(format!(
            "serialize Anthropic request body: {e}"
        )))
    })
}

/// Log and render a send that never got a response, then classify it.
fn report_transport_failure(e: &reqwest::Error, send_started_at: Instant) -> ApiError {
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
    classify_transport_error(e, "Model provider API request failed")
}
