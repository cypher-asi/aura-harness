//! Signing in to boxd: a `BOXD_TOKEN` is used as is; a `BOXD_API_KEY`
//! is exchanged at the console's `POST /api/v1/auth/token` for a JWT
//! that lives one hour.
//!
//! ## Invariants
//!
//! - One [`TokenSource`] serves the whole app: the exchange endpoint is
//!   rate-limited per IP, and the cached JWT is refreshed
//!   [`REFRESH_BEFORE`] ahead of its expiry, so concurrent calls share
//!   one exchange.
//! - The JWT is held in memory only, zeroized on drop, and never logged.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aura_config::{BoxdApiKey, BoxdCredential, BoxdToken};
use aura_core_types::ErrorChain;
use serde::Deserialize;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::error::{CloudError, CloudResult};

/// Refresh a cached token this long before it expires.
pub const REFRESH_BEFORE: Duration = Duration::from_secs(10 * 60);

/// Limit on one exchange call.
pub const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(20);

/// The exchange endpoint for a gRPC base URL: `https://app.<host>` for
/// boxd's own cluster (the CLI installer derives it the same way).
#[must_use]
pub fn exchange_url_for(base_url: &str) -> String {
    let without_scheme = base_url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = without_scheme
        .split(['/', ':'])
        .next()
        .unwrap_or(without_scheme);
    // Regional endpoints (`nyc.boxd.sh`) share the home console.
    let console = if host == "boxd.sh" || host.ends_with(".boxd.sh") {
        "app.boxd.sh".to_string()
    } else {
        format!("app.{host}")
    };
    format!("https://{console}/api/v1/auth/token")
}

#[derive(Deserialize)]
struct ExchangeAnswer {
    token: String,
    /// Unix seconds.
    expires_at: u64,
}

struct Cached {
    token: BoxdToken,
    expires_at: u64,
}

/// Hands out a bearer token for boxd calls.
pub struct TokenSource {
    credential: BoxdCredential,
    exchange_url: String,
    http: reqwest::Client,
    cached: Mutex<Option<Cached>>,
}

impl std::fmt::Debug for TokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenSource")
            .field("credential", &self.credential)
            .field("exchange_url", &self.exchange_url)
            .finish_non_exhaustive()
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl TokenSource {
    /// A source for `credential`, exchanging keys at `exchange_url`.
    ///
    /// # Errors
    ///
    /// When the HTTP client cannot be built.
    pub fn new(credential: BoxdCredential, exchange_url: impl Into<String>) -> CloudResult<Self> {
        let http = reqwest::Client::builder()
            .timeout(EXCHANGE_TIMEOUT)
            .build()
            .map_err(|err| {
                CloudError::Auth(format!("could not build HTTP client: {}", ErrorChain(&err)))
            })?;
        Ok(Self {
            credential,
            exchange_url: exchange_url.into(),
            http,
            cached: Mutex::new(None),
        })
    }

    /// The credential's source variable, for status lines.
    #[must_use]
    pub const fn source_var(&self) -> &'static str {
        self.credential.source_var()
    }

    /// A bearer token valid for at least [`REFRESH_BEFORE`] (or the
    /// user's own token as is).
    ///
    /// # Errors
    ///
    /// [`CloudError::Auth`] when the exchange fails or is refused.
    pub async fn bearer(&self) -> CloudResult<BoxdToken> {
        let key = match &self.credential {
            BoxdCredential::Token(token) => return Ok(token.clone()),
            BoxdCredential::ApiKey(key) => key,
        };
        let mut cached = self.cached.lock().await;
        let fresh_until = now_secs() + REFRESH_BEFORE.as_secs();
        if let Some(entry) = cached.as_ref().filter(|c| c.expires_at > fresh_until) {
            return Ok(entry.token.clone());
        }
        let answer = self.exchange(key).await?;
        let expires_at = answer.expires_at;
        let raw = Zeroizing::new(answer.token);
        let token = BoxdToken::new(raw.as_str());
        *cached = Some(Cached {
            token: token.clone(),
            expires_at,
        });
        tracing::debug!(expires_at, "boxd token refreshed");
        Ok(token)
    }

    /// Forget the cached token, e.g. after boxd answered
    /// `UNAUTHENTICATED`.
    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }

    async fn exchange(&self, key: &BoxdApiKey) -> CloudResult<ExchangeAnswer> {
        let body = serde_json::json!({ "api_key": key.expose() });
        let response = self
            .http
            .post(&self.exchange_url)
            .json(&body)
            .send()
            .await
            .map_err(|err| {
                CloudError::Auth(format!("{}: {}", self.exchange_url, ErrorChain(&err)))
            })?;
        let status = response.status();
        if !status.is_success() {
            // The body may echo request details; keep only the status.
            return Err(CloudError::Auth(format!(
                "{} answered {status} for {key}",
                self.exchange_url
            )));
        }
        response.json::<ExchangeAnswer>().await.map_err(|err| {
            CloudError::Auth(format!("unreadable token answer: {}", ErrorChain(&err)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn exchange_urls() {
        assert_eq!(
            exchange_url_for("https://boxd.sh:9443"),
            "https://app.boxd.sh/api/v1/auth/token"
        );
        assert_eq!(
            exchange_url_for("nyc.boxd.sh:9443"),
            "https://app.boxd.sh/api/v1/auth/token"
        );
        assert_eq!(
            exchange_url_for("https://boxd.example.com:9443"),
            "https://app.boxd.example.com/api/v1/auth/token"
        );
    }

    /// A one-shot HTTP server answering every request with `body`,
    /// counting requests.
    async fn server(
        body: &'static str,
        status: &'static str,
    ) -> (String, tokio::task::JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = tokio::spawn(async move {
            let mut served = 0;
            while let Ok(Ok((mut socket, _))) =
                tokio::time::timeout(Duration::from_millis(500), listener.accept()).await
            {
                let mut buf = vec![0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                assert!(request.contains("\"api_key\":\"bxd_"), "{request}");
                let reply = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
                served += 1;
            }
            served
        });
        (format!("http://{addr}/api/v1/auth/token"), handle)
    }

    #[tokio::test]
    async fn a_key_is_exchanged_once_and_cached() {
        let (url, served) = server(
            r#"{"token":"jwt-abc-123456789","expires_at":99999999999,"user_id":"u"}"#,
            "200 OK",
        )
        .await;
        let source = TokenSource::new(
            BoxdCredential::ApiKey(BoxdApiKey::new("bxd_test_key_0123456789")),
            url,
        )
        .expect("source");
        let first = source.bearer().await.expect("token");
        let second = source.bearer().await.expect("token");
        assert_eq!(first.expose(), "jwt-abc-123456789");
        assert_eq!(second.expose(), "jwt-abc-123456789");
        assert_eq!(served.await.expect("server"), 1);
    }

    #[tokio::test]
    async fn an_expiring_token_is_refreshed() {
        let soon = now_secs() + 60;
        let body: &'static str = Box::leak(
            format!(r#"{{"token":"jwt-soon-123456789","expires_at":{soon}}}"#).into_boxed_str(),
        );
        let (url, served) = server(body, "200 OK").await;
        let source = TokenSource::new(
            BoxdCredential::ApiKey(BoxdApiKey::new("bxd_test_key_0123456789")),
            url,
        )
        .expect("source");
        source.bearer().await.expect("token");
        source.bearer().await.expect("token");
        assert_eq!(served.await.expect("server"), 2);
    }

    #[tokio::test]
    async fn a_refused_key_does_not_leak() {
        let (url, _served) = server(r#"{"error":"bad key"}"#, "401 Unauthorized").await;
        let source = TokenSource::new(
            BoxdCredential::ApiKey(BoxdApiKey::new("bxd_secret_value_0123456789")),
            url,
        )
        .expect("source");
        let err = source.bearer().await.expect_err("refused");
        let text = err.to_string();
        assert!(text.contains("401"), "{text}");
        assert!(!text.contains("secret_value"), "{text}");
    }

    #[tokio::test]
    async fn a_token_is_used_as_is() {
        let source = TokenSource::new(
            BoxdCredential::Token(BoxdToken::new("given-token-123456")),
            "http://127.0.0.1:1/never",
        )
        .expect("source");
        assert_eq!(
            source.bearer().await.expect("token").expose(),
            "given-token-123456"
        );
    }
}
