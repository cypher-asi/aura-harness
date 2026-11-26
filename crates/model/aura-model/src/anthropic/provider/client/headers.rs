//! Request builder for `/v1/messages`: URL, auth, `anthropic-beta`,
//! caller extra headers and the Aura routing headers.

use crate::anthropic::convert::{
    extended_cache_ttl, request_uses_computer_tool, COMPUTER_USE_BETA, EXTENDED_CACHE_TTL_BETA,
};
use crate::anthropic::{AnthropicProvider, ApiError};
use crate::ModelRequest;
use reqwest::RequestBuilder;

impl AnthropicProvider {
    // The function is infallible today but kept on a `Result` shape
    // because every caller threads it through `?`; converting to a
    // plain return would force the callers to drop or re-wrap the
    // result and the current shape leaves room to surface
    // header/body validation errors without a downstream churn.
    #[allow(clippy::unnecessary_wraps)]
    pub(super) fn build_request(
        &self,
        request_ctx: &ModelRequest,
        model: &str,
        body_bytes: Vec<u8>,
        streaming: bool,
    ) -> Result<RequestBuilder, ApiError> {
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

        let req_builder = self.with_beta_and_extra_headers(req_builder, request_ctx, model);
        let req_builder = with_aura_routing_headers(req_builder, request_ctx);
        Ok(Self::with_openai_cache_headers(
            req_builder,
            request_ctx,
            model,
        ))
    }

    /// Assemble the `anthropic-beta` header. Prompt caching keeps its
    /// exact prior token, and computer-use appends a second token only
    /// when the run carries the `computer` tool — so non-computer-use
    /// requests stay byte-identical. Caller extra headers are added in
    /// order; their `anthropic-beta` values merge into the one header.
    fn with_beta_and_extra_headers(
        &self,
        mut req_builder: RequestBuilder,
        request_ctx: &ModelRequest,
        model: &str,
    ) -> RequestBuilder {
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
        req_builder
    }

    fn with_openai_cache_headers(
        mut req_builder: RequestBuilder,
        request_ctx: &ModelRequest,
        model: &str,
    ) -> RequestBuilder {
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
        req_builder
    }
}

fn with_aura_routing_headers(
    mut req_builder: RequestBuilder,
    request_ctx: &ModelRequest,
) -> RequestBuilder {
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
    req_builder
}
