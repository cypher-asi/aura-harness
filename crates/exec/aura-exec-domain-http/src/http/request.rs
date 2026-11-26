//! Authenticated request helpers shared by every `DomainApi` route:
//! JWT checks, Cloudflare-block retry, and typed JSON verbs.

use anyhow::{anyhow, Context};
use serde::de::DeserializeOwned;
use tracing::{debug, warn};

use super::HttpDomainApi;

const MAX_CLOUDFLARE_RETRIES: u32 = 2;
const CLOUDFLARE_RETRY_BASE_MS: u64 = 1500;

// Tool envelopes display only the outer anyhow error. Include a safe transport
// category there; keeping it solely in the source chain renders just GET <URL>.
pub(super) fn transport_error(method: &str, url: &str, error: reqwest::Error) -> anyhow::Error {
    let reason = if error.is_timeout() {
        "request timed out"
    } else if error.is_connect() {
        "connection failed"
    } else {
        "request failed"
    };
    anyhow::Error::new(error).context(format!("{method} {url}: {reason}"))
}

fn is_cloudflare_block(status: reqwest::StatusCode, body: &str) -> bool {
    (status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::SERVICE_UNAVAILABLE)
        && body.contains("<!DOCTYPE html")
}

impl HttpDomainApi {
    // -------------------------------------------------------------------------
    // JWT helpers (Authorization: Bearer, for /api/ routes)
    // -------------------------------------------------------------------------

    pub(super) fn require_jwt(jwt: Option<&str>) -> anyhow::Result<&str> {
        jwt.ok_or_else(|| anyhow!("JWT required for this operation but not provided — ensure the front-end sends a token in RuntimeRequest.auth_jwt"))
    }

    async fn send_with_retry(
        &self,
        method: &str,
        url: &str,
        jwt: &str,
        body: Option<&serde_json::Value>,
    ) -> anyhow::Result<String> {
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
            let text = resp.text().await?;

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

            let truncated: String = if is_cloudflare_block(status, &text) {
                format!("Cloudflare is blocking requests to {url} — the service may be cold-starting or temporarily unavailable")
            } else {
                text.chars().take(300).collect()
            };
            return Err(anyhow!("HTTP {status}: {truncated}"));
        }
        unreachable!()
    }

    pub(super) async fn api_get<T: DeserializeOwned>(
        &self,
        url: &str,
        jwt: &str,
    ) -> anyhow::Result<T> {
        debug!(url, "HttpDomainApi api GET");
        let text = self.send_with_retry("GET", url, jwt, None).await?;
        serde_json::from_str(&text).with_context(|| format!("parse response from {url}"))
    }

    pub(super) async fn api_post<T: DeserializeOwned>(
        &self,
        url: &str,
        body: &serde_json::Value,
        jwt: &str,
    ) -> anyhow::Result<T> {
        debug!(url, "HttpDomainApi api POST");
        let text = self.send_with_retry("POST", url, jwt, Some(body)).await?;
        serde_json::from_str(&text).with_context(|| format!("parse response from {url}"))
    }

    pub(super) async fn api_put<T: DeserializeOwned>(
        &self,
        url: &str,
        body: &serde_json::Value,
        jwt: &str,
    ) -> anyhow::Result<T> {
        debug!(url, "HttpDomainApi api PUT");
        let text = self.send_with_retry("PUT", url, jwt, Some(body)).await?;
        serde_json::from_str(&text).with_context(|| format!("parse response from {url}"))
    }

    pub(super) async fn api_delete(&self, url: &str, jwt: &str) -> anyhow::Result<()> {
        debug!(url, "HttpDomainApi api DELETE");
        self.send_with_retry("DELETE", url, jwt, None).await?;
        Ok(())
    }
}
