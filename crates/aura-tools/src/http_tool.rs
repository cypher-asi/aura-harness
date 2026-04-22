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
use aura_core::{ToolDefinition, ToolResult};
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
mod tests {
    use super::*;

    use std::net::SocketAddr;
    use std::sync::Arc as StdArc;

    use aura_core::{ActionId, AgentId};
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    use crate::sandbox::Sandbox;
    use crate::ToolConfig;

    /// Captured HTTP request, populated by the mock server.
    #[derive(Default, Debug, Clone)]
    struct Captured {
        method: String,
        path_and_query: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    /// A dead-simple single-connection HTTP/1.1 mock. Reads one
    /// request, captures it, responds with the configured status/body.
    async fn start_mock(
        status: u16,
        response_body: &'static str,
    ) -> (
        SocketAddr,
        StdArc<Mutex<Captured>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let captured: StdArc<Mutex<Captured>> = StdArc::new(Mutex::new(Captured::default()));
        let captured_clone = captured.clone();

        let handle = tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };

            let mut buf = vec![0u8; 16 * 1024];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let raw = String::from_utf8_lossy(&buf[..n]).to_string();

            let mut cap = captured_clone.lock().await;
            if let Some(first_line_end) = raw.find("\r\n") {
                let start_line = &raw[..first_line_end];
                let mut parts = start_line.split_whitespace();
                if let (Some(m), Some(p)) = (parts.next(), parts.next()) {
                    cap.method = m.to_string();
                    cap.path_and_query = p.to_string();
                }
            }
            if let Some(header_end) = raw.find("\r\n\r\n") {
                let headers_block = &raw[raw.find("\r\n").unwrap() + 2..header_end];
                for line in headers_block.split("\r\n") {
                    if let Some((n, v)) = line.split_once(':') {
                        cap.headers
                            .push((n.trim().to_ascii_lowercase(), v.trim().to_string()));
                    }
                }
                cap.body = raw[header_end + 4..].to_string();
            }
            drop(cap);

            let reason = if status == 200 { "OK" } else { "Error" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            let _ = sock.write_all(response.as_bytes()).await;
            let _ = sock.shutdown().await;
        });

        (addr, captured, handle)
    }

    fn ctx() -> ToolContext {
        let dir = std::env::temp_dir();
        ToolContext::new(Sandbox::new(&dir).unwrap(), ToolConfig::default())
    }

    fn dummy_agent() -> (AgentId, ActionId) {
        (AgentId::generate(), ActionId::generate())
    }

    #[tokio::test]
    async fn post_body_includes_json_args_and_bearer_token() {
        let (addr, cap, _h) = start_mock(200, r#"{"ok":true}"#).await;
        let tool = HttpToolDefinition::builder(
            "create_project",
            "Create a project.",
            json!({"type": "object", "properties": {"name": {"type": "string"}}}),
            format!("http://{addr}"),
            "/api/projects",
        )
        .method(HttpMethod::Post)
        .auth(HttpAuthSource::StaticBearer("jwt-xyz".to_string()))
        .try_build()
        .expect("tool must build in tests");

        let _ = dummy_agent();
        let result = tool
            .execute(&ctx(), json!({"name": "hello"}))
            .await
            .unwrap();
        assert!(result.ok);
        assert_eq!(&result.stdout[..], br#"{"ok":true}"#);

        let captured = cap.lock().await;
        assert_eq!(captured.method, "POST");
        assert_eq!(captured.path_and_query, "/api/projects");

        let auth = captured
            .headers
            .iter()
            .find(|(n, _)| n == "authorization")
            .map(|(_, v)| v.clone())
            .expect("Authorization header sent");
        assert_eq!(auth, "Bearer jwt-xyz");

        let content_type = captured
            .headers
            .iter()
            .find(|(n, _)| n == "content-type")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        assert!(content_type.starts_with("application/json"));
        assert!(captured.body.contains("\"name\":\"hello\""));
    }

    #[tokio::test]
    async fn url_placeholder_is_filled_and_removed_from_body() {
        let (addr, cap, _h) = start_mock(200, "{}").await;
        let tool = HttpToolDefinition::builder(
            "update_project",
            "Rename a project.",
            json!({"type": "object"}),
            format!("http://{addr}"),
            "/api/projects/{project_id}",
        )
        .method(HttpMethod::Put)
        .try_build()
        .expect("tool must build in tests");

        let _ = tool
            .execute(&ctx(), json!({"project_id": "abc-123", "name": "new"}))
            .await
            .unwrap();

        let captured = cap.lock().await;
        assert_eq!(captured.method, "PUT");
        assert_eq!(captured.path_and_query, "/api/projects/abc-123");
        assert!(captured.body.contains("\"name\":\"new\""));
        assert!(!captured.body.contains("project_id"));
    }

    #[tokio::test]
    async fn get_promotes_remaining_args_to_query_params() {
        let (addr, cap, _h) = start_mock(200, "[]").await;
        let tool = HttpToolDefinition::builder(
            "list_projects",
            "List projects in the org.",
            json!({"type": "object"}),
            format!("http://{addr}"),
            "/api/orgs/{org_id}/projects",
        )
        .method(HttpMethod::Get)
        .try_build()
        .expect("tool must build in tests");

        let _ = tool
            .execute(
                &ctx(),
                json!({"org_id": "o-1", "archived": false, "limit": 10}),
            )
            .await
            .unwrap();

        let captured = cap.lock().await;
        assert_eq!(captured.method, "GET");
        assert!(captured
            .path_and_query
            .starts_with("/api/orgs/o-1/projects?"));
        assert!(captured.path_and_query.contains("archived=false"));
        assert!(captured.path_and_query.contains("limit=10"));
    }

    #[tokio::test]
    async fn non_2xx_maps_to_tool_result_failure() {
        let (addr, _cap, _h) = start_mock(500, r#"{"error":"boom"}"#).await;
        let tool = HttpToolDefinition::builder(
            "archive_project",
            "Archive a project.",
            json!({"type": "object"}),
            format!("http://{addr}"),
            "/api/projects/{project_id}/archive",
        )
        .method(HttpMethod::Post)
        .try_build()
        .expect("tool must build in tests");

        let result = tool
            .execute(&ctx(), json!({"project_id": "p-1"}))
            .await
            .unwrap();
        assert!(!result.ok);
        assert_eq!(result.exit_code, Some(500));
        assert_eq!(&result.stderr[..], br#"{"error":"boom"}"#);
        assert_eq!(
            result.metadata.get("http_status").map(String::as_str),
            Some("500")
        );
    }

    #[tokio::test]
    async fn dynamic_auth_source_is_consulted_per_call() {
        let (addr, cap, _h) = start_mock(200, "{}").await;
        let token_slot: StdArc<std::sync::Mutex<Option<String>>> =
            StdArc::new(std::sync::Mutex::new(Some("dyn-token".to_string())));
        let token_slot_for_tool = token_slot.clone();

        let tool = HttpToolDefinition::builder(
            "get_org",
            "Fetch an org.",
            json!({"type": "object"}),
            format!("http://{addr}"),
            "/api/orgs/{org_id}",
        )
        .method(HttpMethod::Get)
        .auth(HttpAuthSource::Dynamic(Arc::new(move || {
            token_slot_for_tool.lock().unwrap().clone()
        })))
        .try_build()
        .expect("tool must build in tests");

        let _ = tool
            .execute(&ctx(), json!({"org_id": "o-1"}))
            .await
            .unwrap();

        let captured = cap.lock().await;
        let auth = captured
            .headers
            .iter()
            .find(|(n, _)| n == "authorization")
            .map(|(_, v)| v.clone())
            .expect("auth header");
        assert_eq!(auth, "Bearer dyn-token");
    }

    #[tokio::test]
    async fn definition_reflects_configured_fields() {
        let tool = HttpToolDefinition::builder(
            "create_spec",
            "Draft a spec.",
            json!({"type": "object"}),
            "http://example",
            "/api/specs",
        )
        .method(HttpMethod::Post)
        .eager_input_streaming(true)
        .try_build()
        .expect("tool must build in tests");

        let def = tool.definition();
        assert_eq!(def.name, "create_spec");
        assert_eq!(def.description, "Draft a spec.");
        assert_eq!(def.eager_input_streaming, Some(true));
    }

    #[test]
    fn path_encoding_handles_spaces_and_unicode() {
        assert_eq!(urlencode_path("a b"), "a%20b");
        assert_eq!(urlencode_path("π"), "%CF%80");
        assert_eq!(urlencode_path("safe-123._~"), "safe-123._~");
    }
}
