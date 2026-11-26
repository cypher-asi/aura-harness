mod headers;
mod send;

use super::body_cap::fit_body_under_cap;
use crate::anthropic::AnthropicProvider;
use crate::ModelRequest;
use std::time::Duration;
use tracing::warn;

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
}
