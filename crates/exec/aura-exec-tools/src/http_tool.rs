//! Generic HTTP tool primitive.
//!
//! [`HttpToolDefinition`] wraps an HTTP endpoint as a harness [`Tool`]
//! so domain-specific aura-os operations (projects, specs, tasks, etc.)
//! can be exposed to harness agents without any per-tool Rust glue:
//! each domain endpoint becomes one `HttpToolDefinition` at session
//! boot.
//!
//! Design choices for phase 2 of the super-agent unification plan:
//!
//! - **Stateless.** The tool owns a base URL, an HTTP method, a
//!   description + input schema, and an auth source. It does *not*
//!   hold per-session state; the harness constructs one tool instance
//!   per session (or per agent) and binds a session-scoped auth source
//!   to it.
//! - **Pluggable auth.** [`HttpAuthSource`] carries either a static
//!   bearer token, no auth, or a dynamic closure that looks up the
//!   caller's JWT at the moment of the call. Phase 3 will plug the
//!   per-session JWT through the dynamic variant.
//! - **URL templating.** `{arg}` placeholders in `endpoint` are
//!   substituted from the tool's JSON arguments before the request is
//!   sent (and the arg is removed from the JSON body). Matches the way
//!   aura-os endpoints are shaped (e.g. `/api/projects/{project_id}`).
//! - **Failure contract.** Non-2xx responses become
//!   [`ToolResult::failure`] with the response body as stderr; network
//!   failures return [`ToolError::CommandFailed`].
//!
//! Unit tests spin up an [`axum`]-free in-process mock using
//! [`reqwest`]'s own client against a TCP loopback server configured
//! per test.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use aura_core_types::{ToolDefinition, ToolResult};
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION};
use reqwest::Method;
use serde_json::Value;

use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};

/// Default request-total timeout for HTTP tool calls (Wave 5 / T2.4).
const HTTP_TOOL_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Default connect-phase timeout for HTTP tool calls.
const HTTP_TOOL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default cap on the response body we will hold in memory (Wave 5 / T2.5).
/// Requests exceeding this return [`ToolError::SizeLimitExceeded`] instead
/// of being silently truncated. 5 MiB covers all aura-os domain payloads
/// we know about today; callers with pathological needs can swap in a
/// custom client + read path in the future.
const HTTP_TOOL_MAX_RESPONSE_BYTES: usize = 5 * 1024 * 1024;

/// Shared default client — built once, lazily, with the request/connect
/// timeouts above. Using `Client::default()` previously meant every http
/// tool had an unbounded request horizon, which is how slow endpoints
/// could hang an entire agent turn. (Wave 5 / T2.4.)
///
/// We use `OnceLock` + a fallible accessor ([`default_http_tool_client`])
/// rather than `Lazy` + `.expect(...)` so that catastrophic
/// `reqwest::Client::builder().build()` failures (TLS backend init,
/// etc.) surface as a [`ToolError`] instead of a process-wide panic
/// at module-load time.
static DEFAULT_HTTP_TOOL_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Return a clone of the process-wide default HTTP tool client.
///
/// Builds the client on first call with the timeouts declared above.
/// Returns `Err(ToolError::CommandFailed)` if `reqwest` cannot construct
/// a client (essentially TLS backend init failure) — callers propagate
/// this as any other recoverable tool error.
fn default_http_tool_client() -> Result<reqwest::Client, ToolError> {
    if let Some(c) = DEFAULT_HTTP_TOOL_CLIENT.get() {
        return Ok(c.clone());
    }
    let client = reqwest::Client::builder()
        .timeout(HTTP_TOOL_REQUEST_TIMEOUT)
        .connect_timeout(HTTP_TOOL_CONNECT_TIMEOUT)
        .build()
        .map_err(|e| {
            ToolError::CommandFailed(format!("failed to build default HTTP tool client: {e}"))
        })?;
    // set() may race with another caller; either way, whichever client
    // wins is fine — both have identical config.
    let _ = DEFAULT_HTTP_TOOL_CLIENT.set(client.clone());
    Ok(client)
}

/// HTTP methods supported by [`HttpToolDefinition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

impl HttpMethod {
    fn as_reqwest(self) -> Method {
        match self {
            Self::Get => Method::GET,
            Self::Post => Method::POST,
            Self::Put => Method::PUT,
            Self::Delete => Method::DELETE,
            Self::Patch => Method::PATCH,
        }
    }

    /// Whether this method conventionally accepts a request body.
    const fn has_body(self) -> bool {
        matches!(self, Self::Post | Self::Put | Self::Patch)
    }
}

/// Source of the `Authorization` header attached to HTTP tool calls.
#[derive(Clone)]
pub enum HttpAuthSource {
    /// Do not attach an `Authorization` header.
    None,
    /// Static `Bearer <token>`.
    StaticBearer(String),
    /// Per-call lookup. Useful when the harness plumbs a session JWT
    /// through a shared state.
    Dynamic(Arc<dyn Fn() -> Option<String> + Send + Sync>),
}

impl std::fmt::Debug for HttpAuthSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "HttpAuthSource::None"),
            Self::StaticBearer(_) => write!(f, "HttpAuthSource::StaticBearer(...)"),
            Self::Dynamic(_) => write!(f, "HttpAuthSource::Dynamic(...)"),
        }
    }
}

impl HttpAuthSource {
    fn bearer_token(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::StaticBearer(t) => Some(t.clone()),
            Self::Dynamic(f) => f(),
        }
    }
}

/// A harness [`Tool`] that proxies to an HTTP endpoint.
///
/// Construct with [`HttpToolDefinition::builder`].
#[derive(Debug, Clone)]
pub struct HttpToolDefinition {
    name: String,
    description: String,
    input_schema: Value,
    eager_input_streaming: bool,
    base_url: String,
    endpoint: String,
    method: HttpMethod,
    auth: HttpAuthSource,
    static_headers: HeaderMap,
    client: reqwest::Client,
}

/// Builder for [`HttpToolDefinition`].
#[derive(Debug)]
pub struct HttpToolDefinitionBuilder {
    name: String,
    description: String,
    input_schema: Value,
    eager_input_streaming: bool,
    base_url: String,
    endpoint: String,
    method: HttpMethod,
    auth: HttpAuthSource,
    static_headers: HeaderMap,
    client: Option<reqwest::Client>,
}

impl HttpToolDefinition {
    /// Start building an HTTP tool with the required fields.
    #[must_use]
    pub fn builder(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
        base_url: impl Into<String>,
        endpoint: impl Into<String>,
    ) -> HttpToolDefinitionBuilder {
        HttpToolDefinitionBuilder {
            name: name.into(),
            description: description.into(),
            input_schema,
            eager_input_streaming: false,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            endpoint: endpoint.into(),
            method: HttpMethod::Post,
            auth: HttpAuthSource::None,
            static_headers: HeaderMap::new(),
            client: None,
        }
    }

    /// Name exposed to the model.
    #[must_use]
    pub fn tool_name(&self) -> &str {
        &self.name
    }

    /// Configured base URL (no trailing slash).
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Configured endpoint (may contain `{param}` placeholders).
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Render the full URL for a given arguments map. Placeholders of
    /// the form `{arg}` are pulled from `args` and URL-path-escaped;
    /// consumed keys are returned so callers can remove them from the
    /// body.
    fn build_url(&self, args: &Value) -> Result<(String, Vec<String>), ToolError> {
        let mut consumed = Vec::new();
        let mut out = self.endpoint.clone();
        while let Some(start) = out.find('{') {
            let Some(rel_end) = out[start..].find('}') else {
                return Err(ToolError::InvalidArguments(format!(
                    "unterminated placeholder in endpoint: {}",
                    self.endpoint
                )));
            };
            let end = start + rel_end;
            let key = out[start + 1..end].to_string();
            let raw = args.get(&key).ok_or_else(|| {
                ToolError::InvalidArguments(format!("missing argument for placeholder {{{key}}}"))
            })?;
            let value = match raw {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => {
                    return Err(ToolError::InvalidArguments(format!(
                        "placeholder {{{key}}} must be string/number/bool, got {raw}"
                    )));
                }
            };
            let encoded = urlencode_path(&value);
            out.replace_range(start..=end, &encoded);
            consumed.push(key);
        }
        let url = format!("{}{}", self.base_url, ensure_leading_slash(&out));
        Ok((url, consumed))
    }
}

impl HttpToolDefinitionBuilder {
    #[must_use]
    pub fn method(mut self, method: HttpMethod) -> Self {
        self.method = method;
        self
    }

    #[must_use]
    pub fn auth(mut self, auth: HttpAuthSource) -> Self {
        self.auth = auth;
        self
    }

    #[must_use]
    pub fn eager_input_streaming(mut self, enabled: bool) -> Self {
        self.eager_input_streaming = enabled;
        self
    }

    /// Attach a static header sent with every request. Invalid names
    /// or values are silently ignored — callers pre-validate at config
    /// time, not per-call.
    #[must_use]
    pub fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        let raw_value: String = value.into();
        if let Ok(v) = HeaderValue::from_str(&raw_value) {
            self.static_headers.insert(HeaderName::from_static(name), v);
        }
        self
    }

    #[must_use]
    pub fn client(mut self, client: reqwest::Client) -> Self {
        self.client = Some(client);
        self
    }

    /// Finalize the builder.
    ///
    /// Returns an error if no client was supplied and the process-wide
    /// default client cannot be built (see
    /// [`default_http_tool_client`]).
    pub fn try_build(self) -> Result<HttpToolDefinition, ToolError> {
        // Prefer a caller-supplied client; otherwise share the process-wide
        // default that carries sane timeouts. (Wave 5 / T2.4.)
        let client = match self.client {
            Some(c) => c,
            None => default_http_tool_client()?,
        };
        Ok(HttpToolDefinition {
            name: self.name,
            description: self.description,
            input_schema: self.input_schema,
            eager_input_streaming: self.eager_input_streaming,
            base_url: self.base_url,
            endpoint: self.endpoint,
            method: self.method,
            auth: self.auth,
            static_headers: self.static_headers,
            client,
        })
    }
}

#[async_trait]
impl Tool for HttpToolDefinition {
    fn name(&self) -> &str {
        &self.name
    }

    fn definition(&self) -> ToolDefinition {
        let mut def = ToolDefinition::new(
            self.name.clone(),
            self.description.clone(),
            self.input_schema.clone(),
        );
        if self.eager_input_streaming {
            def.eager_input_streaming = Some(true);
        }
        def
    }

    async fn execute(&self, _ctx: &ToolContext, args: Value) -> Result<ToolResult, ToolError> {
        let (url, consumed) = self.build_url(&args)?;

        // Remove placeholder-consumed keys from the body.
        let mut body = args.clone();
        if self.method.has_body() {
            if let Some(obj) = body.as_object_mut() {
                for key in &consumed {
                    obj.remove(key);
                }
            }
        }

        let mut req = self.client.request(self.method.as_reqwest(), &url);
        for (name, value) in &self.static_headers {
            req = req.header(name, value);
        }
        if let Some(token) = self.auth.bearer_token() {
            match HeaderValue::from_str(&format!("Bearer {token}")) {
                Ok(v) => {
                    req = req.header(AUTHORIZATION, v);
                }
                Err(e) => {
                    return Err(ToolError::InvalidArguments(format!(
                        "invalid bearer token for tool {}: {e}",
                        self.name
                    )));
                }
            }
        }
        if self.method.has_body() {
            req = req.json(&body);
        } else if let Some(obj) = args.as_object() {
            // For GET/DELETE, promote remaining scalar args to query params.
            let params: Vec<(&str, String)> = obj
                .iter()
                .filter(|(k, _)| !consumed.contains(k))
                .filter_map(|(k, v)| match v {
                    Value::String(s) => Some((k.as_str(), s.clone())),
                    Value::Number(n) => Some((k.as_str(), n.to_string())),
                    Value::Bool(b) => Some((k.as_str(), b.to_string())),
                    _ => None,
                })
                .collect();
            if !params.is_empty() {
                req = req.query(&params);
            }
        }

        let resp = req
            .send()
            .await
            .map_err(|e| ToolError::CommandFailed(format!("http request failed: {e}")))?;
        let status = resp.status();
        let body = read_bounded_body(resp, HTTP_TOOL_MAX_RESPONSE_BYTES).await?;

        if status.is_success() {
            Ok(ToolResult::success(self.name.clone(), body))
        } else {
            let mut fail = ToolResult::failure(self.name.clone(), body);
            fail.exit_code = Some(i32::from(status.as_u16()));
            fail.metadata
                .insert("http_status".to_string(), status.as_u16().to_string());
            Ok(fail)
        }
    }
}

/// Stream a response body into memory, enforcing a byte cap.
///
/// Using `bytes_stream()` instead of `resp.bytes()` means we stop reading
/// as soon as we cross the limit — a remote peer cannot blow up our
/// memory simply by returning a multi-GiB payload. Returns
/// [`ToolError::SizeLimitExceeded`] when the running total crosses `limit`.
/// (Wave 5 / T2.5.)
async fn read_bounded_body(
    resp: reqwest::Response,
    limit: usize,
) -> Result<bytes::Bytes, ToolError> {
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| ToolError::CommandFailed(format!("read response body: {e}")))?;
        if buf.len().saturating_add(chunk.len()) > limit {
            return Err(ToolError::SizeLimitExceeded {
                actual: buf.len() + chunk.len(),
                limit,
            });
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(bytes::Bytes::from(buf))
}

fn ensure_leading_slash(s: &str) -> String {
    if s.starts_with('/') {
        s.to_string()
    } else {
        format!("/{s}")
    }
}

/// Minimal path-segment encoder: percent-encodes a small conservative
/// set of reserved characters. Sufficient for IDs / slugs used in the
/// aura-os domain endpoints.
fn urlencode_path(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '0'..='9' | 'A'..='Z' | 'a'..='z' | '-' | '_' | '.' | '~' => out.push(ch),
            _ => {
                let mut buf = [0u8; 4];
                for b in ch.encode_utf8(&mut buf).as_bytes() {
                    let _ = write!(&mut out, "%{b:02X}");
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
