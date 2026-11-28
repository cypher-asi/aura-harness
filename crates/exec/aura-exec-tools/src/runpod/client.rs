//! The HTTP client every Runpod tool goes through: the bearer key, the
//! timeouts, the response byte cap and the mapping of HTTP failures to
//! [`RunpodError`].

use super::output::truncate_chars;
use super::{ApiKeySource, RunpodError, RunpodSettings};
use aura_core_types::ErrorChain;
use reqwest::{Method, Response, StatusCode};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// Largest JSON response read, in bytes.
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Largest error body read, in bytes.
const MAX_ERROR_BYTES: usize = 16 * 1024;

/// Longest error detail quoted from Runpod, in characters.
const MAX_DETAIL_CHARS: usize = 300;

const USER_AGENT: &str = concat!(
    "aura-harness/",
    env!("CARGO_PKG_VERSION"),
    " (runpod tools)"
);

/// Which Runpod API a request goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Api {
    /// `api.runpod.io`: pods, catalog, billing, account.
    Control,
    /// `api.runpod.ai`: serverless jobs.
    Serverless,
}

/// One request, named by the call it makes for error messages.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub call: String,
    pub method: Method,
    pub api: Api,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Option<Value>,
    /// Overrides [`RunpodSettings::request_timeout`].
    pub timeout: Option<Duration>,
}

impl Request {
    pub(crate) fn new(call: &str, method: Method, api: Api, path: String) -> Self {
        Self {
            call: call.to_string(),
            method,
            api,
            path,
            query: Vec::new(),
            body: None,
            timeout: None,
        }
    }

    pub(crate) fn query(mut self, name: &str, value: &impl ToString) -> Self {
        self.query.push((name.to_string(), value.to_string()));
        self
    }

    pub(crate) fn body(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }

    pub(crate) const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// A Runpod HTTP client. Clones share one connection pool.
#[derive(Clone)]
pub struct RunpodClient {
    http: reqwest::Client,
    settings: Arc<RunpodSettings>,
    key: Arc<dyn ApiKeySource>,
}

impl std::fmt::Debug for RunpodClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunpodClient")
            .field("api_base", &self.settings.api_base)
            .field("key", &self.key.describe())
            .finish_non_exhaustive()
    }
}

impl RunpodClient {
    /// A client sending to `settings`' URLs with the key from `key`.
    ///
    /// # Errors
    /// [`RunpodError::Client`] when the HTTP client can't be built.
    pub fn new(settings: RunpodSettings, key: Arc<dyn ApiKeySource>) -> Result<Self, RunpodError> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(settings.connect_timeout)
            .build()
            .map_err(|e| RunpodError::Client(ErrorChain(&e).to_string()))?;
        Ok(Self {
            http,
            settings: Arc::new(settings),
            key,
        })
    }

    /// A client sending through `http`, an existing (shared) client.
    /// Every request still sets its own timeout from `settings`.
    #[must_use]
    pub fn with_http(
        http: reqwest::Client,
        settings: RunpodSettings,
        key: Arc<dyn ApiKeySource>,
    ) -> Self {
        Self {
            http,
            settings: Arc::new(settings),
            key,
        }
    }

    /// The settings the client was built with.
    #[must_use]
    pub fn settings(&self) -> &RunpodSettings {
        &self.settings
    }

    /// Send `request` and parse the JSON answer; an empty body is `null`.
    pub(crate) async fn json(&self, request: Request) -> Result<Value, RunpodError> {
        let timeout = self.timeout_of(&request);
        let response = self.send(&request, timeout).await?;
        let (bytes, complete) =
            read_capped(&request.call, response, MAX_RESPONSE_BYTES, None, timeout).await?;
        if !complete {
            return Err(RunpodError::bad_response(
                &request.call,
                format!("the response is over {MAX_RESPONSE_BYTES} bytes"),
            ));
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| RunpodError::bad_response(&request.call, format!("not JSON: {e}")))
    }

    /// Send `request` and read its streamed body for at most `read_for`
    /// or `max_bytes`, whichever comes first.
    pub(crate) async fn stream_text(
        &self,
        request: Request,
        read_for: Duration,
        max_bytes: usize,
    ) -> Result<String, RunpodError> {
        let timeout = self.timeout_of(&request) + read_for;
        let response = self.send(&request, timeout).await?;
        let deadline = tokio::time::Instant::now() + read_for;
        let (bytes, _) =
            read_capped(&request.call, response, max_bytes, Some(deadline), timeout).await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn timeout_of(&self, request: &Request) -> Duration {
        request.timeout.unwrap_or(self.settings.request_timeout)
    }

    async fn send(&self, request: &Request, timeout: Duration) -> Result<Response, RunpodError> {
        let key = self.key.api_key().ok_or_else(|| RunpodError::MissingKey {
            call: request.call.clone(),
            location: self.key.describe(),
        })?;
        let base = match request.api {
            Api::Control => &self.settings.api_base,
            Api::Serverless => &self.settings.serverless_base,
        };
        let mut builder = self
            .http
            .request(request.method.clone(), format!("{base}{}", request.path))
            .bearer_auth(key.expose())
            .timeout(timeout)
            .query(&request.query);
        if let Some(body) = &request.body {
            builder = builder.json(body);
        }
        tracing::debug!(call = %request.call, "runpod request");
        let response = builder
            .send()
            .await
            .map_err(|e| RunpodError::from_reqwest(&request.call, &e, timeout))?;
        check_status(&request.call, response, timeout).await
    }
}

/// Read `response`'s body up to `max_bytes`, stopping at `deadline` if
/// set. Returns the bytes and whether the body ended within the cap.
async fn read_capped(
    call: &str,
    mut response: Response,
    max_bytes: usize,
    deadline: Option<tokio::time::Instant>,
    timeout: Duration,
) -> Result<(Vec<u8>, bool), RunpodError> {
    let mut bytes = Vec::new();
    loop {
        let next = match deadline {
            Some(at) => match tokio::time::timeout_at(at, response.chunk()).await {
                Ok(next) => next,
                Err(_) => return Ok((bytes, false)),
            },
            None => response.chunk().await,
        };
        match next.map_err(|e| RunpodError::from_reqwest(call, &e, timeout))? {
            None => return Ok((bytes, true)),
            Some(chunk) => {
                let room = max_bytes.saturating_sub(bytes.len());
                bytes.extend_from_slice(&chunk[..chunk.len().min(room)]);
                if chunk.len() > room {
                    return Ok((bytes, false));
                }
            }
        }
    }
}

/// Pass a successful response through; turn any other into the error
/// that says what went wrong.
async fn check_status(
    call: &str,
    response: Response,
    timeout: Duration,
) -> Result<Response, RunpodError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let call = call.to_string();
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(RunpodError::Unauthorized {
            call,
            status: status.as_u16(),
        });
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        let retry = rate_limit_reset(&response)
            .map(|secs| format!(" (it resets in {secs}s)"))
            .unwrap_or_default();
        return Err(RunpodError::RateLimited { call, retry });
    }
    let (body, _) = read_capped(&call, response, MAX_ERROR_BYTES, None, timeout)
        .await
        .unwrap_or_default();
    let detail = problem_detail(&body);
    if status == StatusCode::NOT_FOUND {
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        };
        return Err(RunpodError::NotFound { call, detail });
    }
    Err(RunpodError::Api {
        call,
        status: status.as_u16(),
        detail: if detail.is_empty() {
            "no detail given".to_string()
        } else {
            detail
        },
    })
}

/// `title: detail` of a problem+json body, else its text, shortened.
fn problem_detail(body: &[u8]) -> String {
    let text = match serde_json::from_slice::<Value>(body) {
        Ok(problem) => {
            let field = |name: &str| problem.get(name).and_then(Value::as_str).unwrap_or("");
            match (field("title"), field("detail")) {
                ("", "") => problem.to_string(),
                (title, "") => title.to_string(),
                ("", detail) => detail.to_string(),
                (title, detail) => format!("{title}: {detail}"),
            }
        }
        Err(_) => String::from_utf8_lossy(body).trim().to_string(),
    };
    truncate_chars(&text, MAX_DETAIL_CHARS)
}

/// Seconds until the quota resets, from `RateLimit: "default";r=0;t=30`
/// or `Retry-After: 30`.
fn rate_limit_reset(response: &Response) -> Option<u64> {
    let header = |name: &str| response.headers().get(name)?.to_str().ok();
    if let Some(value) = header("ratelimit") {
        let reset = value
            .split(';')
            .find_map(|part| part.trim().strip_prefix("t="))
            .and_then(|t| t.trim().parse().ok());
        if reset.is_some() {
            return reset;
        }
    }
    header("retry-after").and_then(|v| v.trim().parse().ok())
}
