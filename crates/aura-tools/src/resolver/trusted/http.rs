//! Unified provider HTTP send + JSON / form thin wrappers.
//!
//! Phase 1 / T4 collapsed `provider_json_request` and `provider_form_request`
//! into one [`ToolResolver::send_provider_request`] keyed by a body-shape
//! enum so the URL/auth/timeout pipeline lives in exactly one place. The
//! two legacy methods are retained as thin wrappers because every
//! per-integration handler still calls them by name.

use super::super::runtime_headers::{
    apply_runtime_auth, apply_runtime_headers, runtime_url_with_auth,
};
use super::super::TRUSTED_PROVIDER_REQUEST_TIMEOUT;
use super::ToolResolver;
use crate::error::ToolError;
use aura_core::{InstalledToolRuntimeIntegration, InstalledToolRuntimeProviderExecution};
use reqwest::Method;
use serde_json::Value;

/// Body variants accepted by [`ToolResolver::send_provider_request`].
///
/// Collapses what used to be two parallel helpers (`provider_json_request`
/// / `provider_form_request`) into one — the body shape is the only thing
/// that differs between them.
pub(super) enum ProviderRequestBody {
    None,
    Json(Value),
    Form(Vec<(String, String)>),
}

impl ToolResolver {
    pub(super) async fn provider_json_request(
        &self,
        method: Method,
        url: &str,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        body: Option<Value>,
    ) -> Result<Value, ToolError> {
        self.send_provider_request(
            method,
            url,
            provider,
            integration,
            body.map_or(ProviderRequestBody::None, ProviderRequestBody::Json),
        )
        .await
    }

    pub(super) async fn provider_form_request(
        &self,
        method: Method,
        url: &str,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        body: Vec<(String, String)>,
    ) -> Result<Value, ToolError> {
        self.send_provider_request(
            method,
            url,
            provider,
            integration,
            ProviderRequestBody::Form(body),
        )
        .await
    }

    /// Unified trusted-provider HTTP send.
    ///
    /// Builds the URL, applies runtime headers + auth, attaches the
    /// requested body (JSON, form-encoded, or none), and decodes the
    /// response as JSON. Identical timeout/error handling for both
    /// JSON and form bodies — extracted from `provider_json_request`
    /// and `provider_form_request` (Phase 1 dedup).
    async fn send_provider_request(
        &self,
        method: Method,
        url: &str,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        body: ProviderRequestBody,
    ) -> Result<Value, ToolError> {
        let final_url = runtime_url_with_auth(url, integration)?;
        let mut request = self
            .http_client
            .request(method, final_url)
            // Per-request ceiling on trusted-provider calls so a slow
            // integration can't stall the whole turn. (Wave 5 / T2.3.)
            .timeout(TRUSTED_PROVIDER_REQUEST_TIMEOUT);
        request = apply_runtime_headers(request, &provider.static_headers)?;
        request = apply_runtime_auth(request, integration);
        request = match body {
            ProviderRequestBody::None => request,
            ProviderRequestBody::Json(value) => request.json(&value),
            ProviderRequestBody::Form(fields) => request.form(&fields),
        };
        let response = request
            .send()
            .await
            .map_err(|e| ToolError::ExternalToolError(format!("provider request failed: {e}")))?;
        let status = response.status();
        let text = response.text().await.map_err(|e| {
            ToolError::ExternalToolError(format!("reading provider response failed: {e}"))
        })?;
        if !status.is_success() {
            return Err(ToolError::ExternalToolError(format!(
                "provider request failed with {status}: {text}"
            )));
        }
        serde_json::from_str(&text).map_err(|e| {
            ToolError::ExternalToolError(format!("provider returned invalid JSON: {e}"))
        })
    }
}
