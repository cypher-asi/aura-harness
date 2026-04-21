//! Header merging and auth-injection helpers for trusted-provider HTTP
//! calls.
//!
//! Split out of `resolver.rs` in Wave 6 / T4 so the trusted-runtime
//! submodule can stay focused on spec execution. Keep the canonical
//! `Accept: application/json` + `User-Agent: aura-harness` defaults in
//! sync with the tests in `resolver_tests.rs`.

use crate::error::ToolError;
use aura_core::{InstalledToolRuntimeAuth, InstalledToolRuntimeIntegration};
use reqwest::header::{HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use reqwest::{RequestBuilder, Url};
use std::collections::HashMap;

pub(super) fn apply_runtime_headers(
    mut request: RequestBuilder,
    headers: &HashMap<String, String>,
) -> Result<RequestBuilder, ToolError> {
    request = request.header(ACCEPT, "application/json");
    request = request.header(CONTENT_TYPE, "application/json");
    request = request.header(USER_AGENT, "aura-harness");
    for (name, value) in headers {
        let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
            ToolError::ExternalToolError(format!("invalid runtime header name `{name}`: {e}"))
        })?;
        let header_value = HeaderValue::from_str(value).map_err(|e| {
            ToolError::ExternalToolError(format!("invalid runtime header value for `{name}`: {e}"))
        })?;
        request = request.header(header_name, header_value);
    }
    Ok(request)
}

pub(super) fn apply_runtime_auth(
    mut request: RequestBuilder,
    integration: &InstalledToolRuntimeIntegration,
) -> RequestBuilder {
    match &integration.auth {
        InstalledToolRuntimeAuth::None | InstalledToolRuntimeAuth::QueryParam { .. } => {}
        InstalledToolRuntimeAuth::AuthorizationBearer { token } => {
            request = request.bearer_auth(token);
        }
        InstalledToolRuntimeAuth::AuthorizationRaw { value } => {
            request = request.header(AUTHORIZATION, value);
        }
        InstalledToolRuntimeAuth::Header { name, value } => {
            request = request.header(name, value);
        }
        InstalledToolRuntimeAuth::Basic { username, password } => {
            request = request.basic_auth(username, Some(password));
        }
    }
    request
}

pub(super) fn runtime_url_with_auth(
    url: &str,
    integration: &InstalledToolRuntimeIntegration,
) -> Result<String, ToolError> {
    match &integration.auth {
        InstalledToolRuntimeAuth::QueryParam { name, value } => {
            let mut parsed = Url::parse(url).map_err(|e| {
                ToolError::ExternalToolError(format!("invalid runtime url for query auth: {e}"))
            })?;
            parsed.query_pairs_mut().append_pair(name, value);
            Ok(parsed.to_string())
        }
        _ => Ok(url.to_string()),
    }
}
