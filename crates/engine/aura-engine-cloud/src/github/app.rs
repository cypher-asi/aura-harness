//! [`GithubApp`]: a GitHub App's JWTs, installation lookups and
//! one-repository tokens over GitHub's REST API.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use aura_config::GithubAppConfig;
use aura_core_types::time::now_ms;
use aura_core_types::ErrorChain;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::{
    BotIdentity, GithubError, RepoRef, RepoToken, RepoTokens, API_BASE, API_VERSION,
    ASSUMED_TOKEN_LIFETIME_MS, CALL_TIMEOUT, CONNECT_TIMEOUT, JWT_BACKDATE_SECS, JWT_LIFETIME_SECS,
};

/// A GitHub App over GitHub's REST API.
pub struct GithubApp {
    config: GithubAppConfig,
    api: String,
    http: reqwest::Client,
    /// Installation id of each repository looked up.
    installations: Mutex<HashMap<RepoRef, u64>>,
    identity: Mutex<Option<BotIdentity>>,
    warned_key_mode: AtomicBool,
}

impl std::fmt::Debug for GithubApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubApp")
            .field("app_id", &self.config.app_id)
            .field("api", &self.api)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct Installation {
    id: u64,
}

#[derive(Deserialize)]
struct AccessToken {
    token: String,
    expires_at: Option<String>,
}

#[derive(Deserialize)]
struct App {
    slug: String,
}

#[derive(Deserialize)]
struct Account {
    id: u64,
}

impl GithubApp {
    /// The App `config` names, over [`API_BASE`].
    ///
    /// # Errors
    ///
    /// When the HTTP client can't be built.
    pub fn new(config: GithubAppConfig) -> Result<Self, GithubError> {
        Self::with_api(config, API_BASE)
    }

    /// The App over the API at `api` (a test server).
    ///
    /// # Errors
    ///
    /// When the HTTP client can't be built.
    pub fn with_api(config: GithubAppConfig, api: impl Into<String>) -> Result<Self, GithubError> {
        let http = reqwest::Client::builder()
            .timeout(CALL_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("aura/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|err| GithubError::Transport {
                op: "client setup",
                message: ErrorChain(&err).to_string(),
            })?;
        Ok(Self {
            config,
            api: api.into().trim_end_matches('/').to_string(),
            http,
            installations: Mutex::new(HashMap::new()),
            identity: Mutex::new(None),
            warned_key_mode: AtomicBool::new(false),
        })
    }

    /// The permissions every token is narrowed to.
    #[must_use]
    pub fn permissions(&self) -> Value {
        let mut permissions = json!({"contents": "write", "pull_requests": "write"});
        if self.config.workflows {
            permissions["workflows"] = json!("write");
        }
        permissions
    }

    fn key_error(&self, message: impl Into<String>) -> GithubError {
        GithubError::Key {
            path: self.config.private_key_path.display().to_string(),
            message: message.into(),
        }
    }

    #[cfg(unix)]
    fn warn_if_readable(&self, meta: &std::fs::Metadata) {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o044 != 0
            && !self.warned_key_mode.swap(true, Ordering::Relaxed)
        {
            let path = self.config.private_key_path.display();
            tracing::warn!("{path} holds the GitHub App's private key and other users can read it; run `chmod 600 {path}`");
        }
    }

    #[cfg(not(unix))]
    fn warn_if_readable(&self, _meta: &std::fs::Metadata) {
        let _ = &self.warned_key_mode;
    }

    /// A fresh App JWT: RS256, issued a minute ago, nine minutes left.
    async fn app_jwt(&self) -> Result<Zeroizing<String>, GithubError> {
        let path = &self.config.private_key_path;
        let pem = Zeroizing::new(
            tokio::fs::read(path)
                .await
                .map_err(|err| self.key_error(format!("could not read it: {err}")))?,
        );
        if let Ok(meta) = tokio::fs::metadata(path).await {
            self.warn_if_readable(&meta);
        }
        let key = EncodingKey::from_rsa_pem(&pem)
            .map_err(|err| self.key_error(format!("not an RSA private key in PEM form ({err})")))?;
        let now = chrono::Utc::now().timestamp();
        let issuer = self
            .config
            .app_id
            .parse::<u64>()
            .map_or_else(|_| json!(self.config.app_id), |id| json!(id));
        let claims = json!({
            "iat": now - JWT_BACKDATE_SECS,
            "exp": now + JWT_LIFETIME_SECS,
            "iss": issuer,
        });
        jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &key)
            .map(Zeroizing::new)
            .map_err(|err| self.key_error(format!("could not sign the App JWT ({err})")))
    }

    /// One call: its status and body. The body may hold a token, so it is
    /// zeroized on drop and never logged.
    async fn call(
        &self,
        op: &'static str,
        request: reqwest::RequestBuilder,
    ) -> Result<(u16, Zeroizing<String>), GithubError> {
        let response = request
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", API_VERSION)
            .send()
            .await
            .map_err(|err| GithubError::Transport {
                op,
                message: ErrorChain(&err).to_string(),
            })?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|err| GithubError::Transport {
                op,
                message: ErrorChain(&err).to_string(),
            })?;
        Ok((status, Zeroizing::new(body)))
    }

    fn api_error(op: &'static str, status: u16, body: &str) -> GithubError {
        let message = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|value| value.get("message")?.as_str().map(str::to_string))
            .unwrap_or_else(|| "no message".to_string());
        GithubError::Api {
            op,
            status,
            message: message.chars().take(300).collect(),
        }
    }

    fn parse<T: serde::de::DeserializeOwned>(
        op: &'static str,
        body: &str,
    ) -> Result<T, GithubError> {
        serde_json::from_str(body).map_err(|err| GithubError::InvalidResponse {
            op,
            message: err.to_string(),
        })
    }

    async fn not_installed(&self, repo: &RepoRef) -> GithubError {
        GithubError::NotInstalled {
            repo: repo.full_name(),
            install_url: self.identity().await.ok().map(|bot| bot.install_url()),
        }
    }

    async fn installation(&self, repo: &RepoRef, jwt: &str) -> Result<Option<u64>, GithubError> {
        if let Some(id) = self.installations.lock().get(repo).copied() {
            return Ok(Some(id));
        }
        const OP: &str = "installation lookup";
        let url = format!(
            "{}/repos/{}/{}/installation",
            self.api, repo.owner, repo.name
        );
        let (status, body) = self.call(OP, self.http.get(url).bearer_auth(jwt)).await?;
        match status {
            200 => {
                let id = Self::parse::<Installation>(OP, &body)?.id;
                self.installations.lock().insert(repo.clone(), id);
                Ok(Some(id))
            }
            404 => Ok(None),
            _ => Err(Self::api_error(OP, status, &body)),
        }
    }
}

#[async_trait]
impl RepoTokens for GithubApp {
    async fn identity(&self) -> Result<BotIdentity, GithubError> {
        if let Some(identity) = self.identity.lock().clone() {
            return Ok(identity);
        }
        const OP: &str = "app lookup";
        let jwt = self.app_jwt().await?;
        let (status, body) = self
            .call(
                OP,
                self.http
                    .get(format!("{}/app", self.api))
                    .bearer_auth(jwt.as_str()),
            )
            .await?;
        if status != 200 {
            return Err(Self::api_error(OP, status, &body));
        }
        let slug = Self::parse::<App>(OP, &body)?.slug;
        if slug.is_empty()
            || !slug
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(GithubError::InvalidResponse {
                op: OP,
                message: format!("app slug {slug:?}"),
            });
        }
        // The bot account is public; its id makes the commit email link.
        let account = format!("{}/users/{slug}%5Bbot%5D", self.api);
        let account_id = match self.call("bot lookup", self.http.get(account)).await {
            Ok((200, body)) => serde_json::from_str::<Account>(&body).ok().map(|a| a.id),
            _ => None,
        };
        let identity = BotIdentity { slug, account_id };
        if identity.account_id.is_some() {
            *self.identity.lock() = Some(identity.clone());
        }
        Ok(identity)
    }

    async fn mint(&self, repo: &RepoRef) -> Result<RepoToken, GithubError> {
        const OP: &str = "token mint";
        let jwt = self.app_jwt().await?;
        let Some(installation) = self.installation(repo, &jwt).await? else {
            return Err(self.not_installed(repo).await);
        };
        let url = format!(
            "{}/app/installations/{installation}/access_tokens",
            self.api
        );
        let request = self.http.post(url).bearer_auth(jwt.as_str()).json(&json!({
            "repositories": [repo.name],
            "permissions": self.permissions(),
        }));
        let (status, body) = self.call(OP, request).await?;
        match status {
            200 | 201 => {
                let AccessToken { token, expires_at } = Self::parse(OP, &body)?;
                let token = Zeroizing::new(token);
                if token.is_empty() {
                    return Err(GithubError::InvalidResponse {
                        op: OP,
                        message: "no token".into(),
                    });
                }
                let expires_at_ms = expires_at
                    .as_deref()
                    .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                    .and_then(|at| u64::try_from(at.timestamp_millis()).ok())
                    .unwrap_or_else(|| now_ms() + ASSUMED_TOKEN_LIFETIME_MS);
                Ok(RepoToken {
                    token,
                    expires_at_ms,
                })
            }
            // The installation is gone, or no longer includes the repo.
            404 | 422 => {
                self.installations.lock().remove(repo);
                Err(self.not_installed(repo).await)
            }
            _ => Err(Self::api_error(OP, status, &body)),
        }
    }
}
