//! Authenticated request helpers shared by every `DomainApi` route:
//! JWT checks, Cloudflare-block retry, and typed JSON verbs.

use aura_exec_traits::domain::{DomainError, DomainResult};
use serde::de::DeserializeOwned;
use tracing::{debug, warn};

use super::HttpDomainApi;

const MAX_CLOUDFLARE_RETRIES: u32 = 2;
const CLOUDFLARE_RETRY_BASE_MS: u64 = 1500;

// Tool envelopes display only the error's message, not its source. Include
// a safe transport category there; keeping it solely in the source chain
// renders just GET <URL>.
pub(super) fn transport_error(method: &str, url: &str, error: reqwest::Error) -> DomainError {
    let reason = if error.is_timeout() {
        "request timed out"
    } else if error.is_connect() {
        "connection failed"
    } else {
        "request failed"
    };
    DomainError::Transport {
        method: method.to_string(),
        url: url.to_string(),
        reason,
        source: Box::new(error),
    }
}

/// The response with the non-success `status` and `body` (the start of
/// the response body).
pub(super) fn status_error(status: reqwest::StatusCode, body: String) -> DomainError {
    DomainError::Status {
        status: status.as_u16(),
        reason: status.canonical_reason(),
        body,
    }
}

/// The response body could not be read.
pub(super) fn body_error(error: reqwest::Error) -> DomainError {
    DomainError::Body(Box::new(error))
}

fn is_cloudflare_block(status: reqwest::StatusCode, body: &str) -> bool {
    (status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::SERVICE_UNAVAILABLE)
        && body.contains("<!DOCTYPE html")
}

/// `text`, the response from `url`, as JSON.
fn decode<T: DeserializeOwned>(url: &str, text: &str) -> DomainResult<T> {
    serde_json::from_str(text).map_err(|source| DomainError::Decode {
        url: url.to_string(),
        source,
    })
}

impl HttpDomainApi {
    // -------------------------------------------------------------------------
    // JWT helpers (Authorization: Bearer, for /api/ routes)
    // -------------------------------------------------------------------------

    pub(super) fn require_jwt(jwt: Option<&str>) -> DomainResult<&str> {
        jwt.ok_or(DomainError::MissingJwt)
    }

    async fn send_with_retry(
        &self,
        method: &str,
        url: &str,
        jwt: &str,
        body: Option<&serde_json::Value>,
    ) -> DomainResult<String> {
        for attempt in 0..=MAX_CLOUDFLARE_RETRIES {
            let req = match method {
                "POST" => self.http.post(url).bearer_auth(jwt),
                "PUT" => self.http.put(url).bearer_auth(jwt),
                "DELETE" => self.http.delete(url).bearer_auth(jwt),
                _ => self.http.get(url).bearer_auth(jwt),
            };
            let req = if let Some(b) = body { req.json(b) } else { req };

            let resp = req
                .send()
                .await
                .map_err(|error| transport_error(method, url, error))?;
            let status = resp.status();
            let text = resp.text().await.map_err(body_error)?;

            if status.is_success() {
                return Ok(text);
            }

            if is_cloudflare_block(status, &text) && attempt < MAX_CLOUDFLARE_RETRIES {
                let backoff = CLOUDFLARE_RETRY_BASE_MS * u64::from(2u32.pow(attempt));
                warn!(
                    url,
                    attempt,
                    backoff_ms = backoff,
                    "Cloudflare block detected, retrying"
                );
                tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
                continue;
            }

            return Err(if is_cloudflare_block(status, &text) {
                DomainError::CloudflareBlocked {
                    status: status.as_u16(),
                    reason: status.canonical_reason(),
                    url: url.to_string(),
                }
            } else {
                status_error(status, text.chars().take(300).collect())
            });
        }
        unreachable!()
    }

    pub(super) async fn api_get<T: DeserializeOwned>(
        &self,
        url: &str,
        jwt: &str,
    ) -> DomainResult<T> {
        debug!(url, "HttpDomainApi api GET");
        let text = self.send_with_retry("GET", url, jwt, None).await?;
        decode(url, &text)
    }

    pub(super) async fn api_post<T: DeserializeOwned>(
        &self,
        url: &str,
        body: &serde_json::Value,
        jwt: &str,
    ) -> DomainResult<T> {
        debug!(url, "HttpDomainApi api POST");
        let text = self.send_with_retry("POST", url, jwt, Some(body)).await?;
        decode(url, &text)
    }

    pub(super) async fn api_put<T: DeserializeOwned>(
        &self,
        url: &str,
        body: &serde_json::Value,
        jwt: &str,
    ) -> DomainResult<T> {
        debug!(url, "HttpDomainApi api PUT");
        let text = self.send_with_retry("PUT", url, jwt, Some(body)).await?;
        decode(url, &text)
    }

    pub(super) async fn api_delete(&self, url: &str, jwt: &str) -> DomainResult<()> {
        debug!(url, "HttpDomainApi api DELETE");
        self.send_with_retry("DELETE", url, jwt, None).await?;
        Ok(())
    }
}
