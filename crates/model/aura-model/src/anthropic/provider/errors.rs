use super::RequestRoutingContext;
use crate::anthropic::ApiError;
use crate::error::ReasonerError;
use crate::ModelContentProfile;
use std::time::Duration;
use tracing::error;

/// Diagnostic metadata extracted alongside the [`ApiError`] return of
/// [`classify_api_error`]. Carries the small string set the outbound
/// failure block needs to render (status / class label / request_id /
/// retry_after / body preview) so the call site can emit a paired
/// `← <status>` block under `aura::console` without having to re-walk
/// the response.
#[derive(Debug, Clone)]
pub(super) struct FailureMeta {
    pub class: &'static str,
    pub status_code: u16,
    pub status_text: String,
    pub request_id: Option<String>,
    pub retry_after_s: Option<u64>,
    pub body_preview: String,
}

pub(super) async fn classify_api_error(
    response: reqwest::Response,
    routing: RequestRoutingContext,
    content_profile: Option<&ModelContentProfile>,
    wire_body_bytes: usize,
) -> (ApiError, FailureMeta) {
    let failure = FailedResponse::read(response).await;
    failure.log(routing);

    if crate::anthropic::is_cloudflare_html(&failure.body) {
        failure.dump_cloudflare_html_if_enabled();
        return failure.into_cloudflare_block(routing, content_profile, wire_body_bytes);
    }

    failure.into_status_error()
}

/// A non-success response with its body buffered and its retry /
/// correlation headers pulled out.
struct FailedResponse {
    status: reqwest::StatusCode,
    header_retry_after: Option<Duration>,
    request_id: Option<String>,
    body: String,
    body_preview: String,
}

impl FailedResponse {
    async fn read(response: reqwest::Response) -> Self {
        let status = response.status();
        let header_retry_after = parse_retry_after_header(response.headers());
        // Pull any quota / request-id headers before consuming the response body so
        // 429/529 failures are easier to correlate with proxy-side logs.
        let header_request_id = response
            .headers()
            .get("x-request-id")
            .or_else(|| response.headers().get("request-id"))
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = response.text().await.unwrap_or_default();
        let request_id = header_request_id.or_else(|| extract_waf_request_id_from_body(&body));
        let body_preview = crate::truncate_body(&body, 200);
        Self {
            status,
            header_retry_after,
            request_id,
            body,
            body_preview,
        }
    }

    fn header_retry_after_s(&self) -> Option<u64> {
        self.header_retry_after.map(|d| d.as_secs())
    }

    fn log(&self, routing: RequestRoutingContext) {
        error!(
            status = %self.status,
            body = %self.body_preview,
            retry_after_s = ?self.header_retry_after_s(),
            request_id = ?self.request_id,
            aura_org_id = routing.org_label(),
            aura_session_id = routing.session_label(),
            "Model provider API error"
        );
    }

    fn dump_cloudflare_html_if_enabled(&self) {
        let Ok(dir) = std::env::var("AURA_DEBUG_CLOUDFLARE_DUMP_DIR") else {
            return;
        };
        let dir_path = std::path::PathBuf::from(&dir);
        if std::fs::create_dir_all(&dir_path).is_err() {
            return;
        }
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let file = dir_path.join(format!("cf-block-{ts}.html"));
        let header_dump = format!(
            "<!-- aura-debug status={} request_id={:?} retry_after_s={:?} -->\n",
            self.status,
            self.request_id,
            self.header_retry_after_s()
        );
        let _ = std::fs::write(&file, format!("{header_dump}{}", self.body));
        error!(
            cloudflare_dump_path = %file.display(),
            "Cloudflare HTML dumped for diagnosis"
        );
    }

    fn into_cloudflare_block(
        self,
        routing: RequestRoutingContext,
        content_profile: Option<&ModelContentProfile>,
        wire_body_bytes: usize,
    ) -> (ApiError, FailureMeta) {
        let status = self.status;
        let request_id_label = self.request_id.as_deref().unwrap_or("unknown");
        let profile_label = content_profile.map_or_else(
            || "profile=unavailable".to_string(),
            ModelContentProfile::summary,
        );
        let err = ApiError::CloudflareBlock {
            message: format!(
                "LLM proxy returned Cloudflare block ({status}; request_id={request_id_label}; \
                 aura_org_id={}; aura_session_id={}; {profile_label})",
                routing.org_label(),
                routing.session_label()
            ),
            wire_body_bytes: Some(wire_body_bytes),
        };
        let retry_after_s = self.header_retry_after_s();
        (err, self.into_meta("cloudflare_block", retry_after_s))
    }

    fn into_status_error(self) -> (ApiError, FailureMeta) {
        let (err, class) = status_error(self.status, &self.body, self.header_retry_after);
        let retry_after_s = match &err {
            ApiError::Overloaded { retry_after, .. } => retry_after.map(|d| d.as_secs()),
            _ => self.header_retry_after_s(),
        };
        (err, self.into_meta(class, retry_after_s))
    }

    fn into_meta(self, class: &'static str, retry_after_s: Option<u64>) -> FailureMeta {
        let status = self.status;
        FailureMeta {
            class,
            status_code: status.as_u16(),
            status_text: status
                .canonical_reason()
                .map_or_else(|| status.to_string(), str::to_string),
            request_id: self.request_id,
            retry_after_s,
            body_preview: self.body_preview,
        }
    }
}

/// Map a non-Cloudflare failure status to its error and class label.
fn status_error(
    status: reqwest::StatusCode,
    body: &str,
    header_retry_after: Option<Duration>,
) -> (ApiError, &'static str) {
    let status_code = status.as_u16();
    match status_code {
        402 => (
            ApiError::InsufficientCredits(provider_api_error_message(status, body)),
            "insufficient_credits",
        ),
        429 | 529 => {
            let body_retry_after = parse_retry_after_from_body(body);
            let retry_after = header_retry_after.or(body_retry_after);
            (
                ApiError::Overloaded {
                    message: provider_api_error_message(status, body),
                    retry_after,
                },
                "rate_limited_429",
            )
        }
        // Axis 2: generic 5xx from the upstream LLM / proxy. Routed
        // through the retry path with bounded exponential backoff so a
        // single provider blip (`500 Internal server error`, `502 Bad
        // gateway`, `503 Service Unavailable` with a non-Cloudflare
        // body, `504 Gateway Timeout`) doesn't immediately surface as
        // a terminal failure to the dev loop. 501/505..=511 are left
        // as `Other` — those are configuration or protocol errors that
        // retrying will not fix.
        500 | 502 | 503 | 504 => (
            ApiError::TransientServer {
                status: status_code,
                message: provider_api_error_message(status, body),
            },
            "upstream_5xx",
        ),
        _ => (
            ApiError::Other(ReasonerError::Api {
                status: status_code,
                message: format!("{status} - {body}"),
            }),
            "other",
        ),
    }
}

pub(super) fn provider_api_error_message(status: reqwest::StatusCode, body: &str) -> String {
    format!("Model provider API error: {status} - {body}")
}

pub(super) fn extract_waf_request_id_from_body(body: &str) -> Option<String> {
    let marker = "Request ID:";
    let start = body.find(marker)? + marker.len();
    let rest = &body[start..];
    let code_start = rest
        .find("<code")
        .and_then(|idx| rest[idx..].find('>').map(|end| idx + end + 1));
    let value_start = code_start.unwrap_or(0);
    let value = rest[value_start..]
        .split('<')
        .next()
        .unwrap_or_default()
        .trim();

    (!value.is_empty()).then(|| value.to_string())
}

/// Parse the HTTP `Retry-After` header. Supports both the seconds form
/// (e.g. `7`) and the HTTP-date form. Returns `None` when absent or
/// unparseable — callers fall back to the body hint or exp-backoff.
pub(super) fn parse_retry_after_header(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
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
pub(super) fn parse_retry_after_from_body(body: &str) -> Option<Duration> {
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
pub(super) fn parse_retry_after_prose(text: &str) -> Option<Duration> {
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

/// Only used before handing a response (or stream) to the caller. reqwest
/// distinguishes transport failures from builder, redirect, status and JSON
/// decoding failures; the latter are permanent and must not be retried.
pub(super) fn is_retryable_transport_error(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect() || error.is_request() || error.is_body()
}

pub(super) fn classify_transport_error(error: &reqwest::Error, context: &str) -> ApiError {
    let retryable = is_retryable_transport_error(error);
    let reason = if error.is_timeout() {
        ReasonerError::Timeout
    } else {
        ReasonerError::Request(format!("{context}: {error}"))
    };
    if retryable {
        ApiError::Transport(reason)
    } else {
        ApiError::Other(reason)
    }
}
