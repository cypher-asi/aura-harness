//! GitHub App tokens for cloud machines.
//!
//! The hub holds a GitHub App's private key ([`aura_config::GithubAppConfig`]).
//! For each repository cloud agents work on, [`GithubApp`] signs a
//! short-lived App JWT, looks up the App's installation on that
//! repository and mints an installation token scoped to that one
//! repository, with `contents` and `pull_requests` write (and
//! `workflows` write only when configured). [`crate::repos::RepoSync`]
//! keeps the token in a boxd host-bound secret; machines hold only its
//! placeholder. Commits, pushes and pull requests are then the App's bot,
//! `<slug>[bot]` ([`BotIdentity`]).
//!
//! ## Invariants
//!
//! - The private key and App JWTs never leave this process and are never
//!   logged; tokens are [`Zeroizing`] and [`RepoToken`]'s `Debug` hides
//!   them. Error messages carry GitHub's `message` field only, never a
//!   response body.
//! - A token is scoped to one repository and to [`GithubApp::permissions`].
//! - Every call has a timeout ([`CALL_TIMEOUT`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use aura_config::GithubAppConfig;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zeroize::Zeroizing;

/// GitHub's REST API.
pub const API_BASE: &str = "https://api.github.com";
/// Limit on one call to GitHub.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(20);
/// Limit on connecting to GitHub.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// App JWTs are issued this far in the past, against clock drift.
const JWT_BACKDATE_SECS: i64 = 60;
/// And expire this far ahead (GitHub allows at most ten minutes).
const JWT_LIFETIME_SECS: i64 = 9 * 60;
/// Assumed lifetime of a token whose expiry GitHub didn't state.
const ASSUMED_TOKEN_LIFETIME_MS: u64 = 50 * 60 * 1000;
/// The REST API version every call asks for.
const API_VERSION: &str = "2022-11-28";

fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

/// A repository on github.com: `owner/name`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RepoRef {
    /// The user or organization.
    pub owner: String,
    /// The repository, without `.git`.
    pub name: String,
}

/// Whether `segment` can be a GitHub owner or repository name.
fn is_name(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

impl RepoRef {
    /// The repository a git remote URL names, when it is on github.com:
    /// `https://github.com/o/r(.git)`, `ssh://git@github.com[:22]/o/r.git`
    /// or `git@github.com:o/r.git`. User-info is ignored. `None` for
    /// other hosts and anything else.
    #[must_use]
    pub fn from_remote(url: &str) -> Option<Self> {
        let url = url.trim();
        let (host, path) = match url.split_once("://") {
            Some((scheme, rest)) => {
                if !matches!(scheme, "https" | "http" | "ssh" | "git") {
                    return None;
                }
                let (authority, path) = rest.split_once('/')?;
                let host = authority
                    .rsplit_once('@')
                    .map_or(authority, |(_, host)| host);
                (host.split(':').next()?, path)
            }
            None => {
                // scp-like `[user@]host:path`.
                let (authority, path) = url.split_once(':')?;
                if authority.contains('/') {
                    return None;
                }
                (
                    authority
                        .rsplit_once('@')
                        .map_or(authority, |(_, host)| host),
                    path,
                )
            }
        };
        if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com")
        {
            return None;
        }
        let path = path.trim_start_matches('/').trim_end_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        let (owner, name) = path.split_once('/')?;
        (is_name(owner) && is_name(name)).then(|| Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    /// `owner/name`.
    #[must_use]
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// The https remote machines push to.
    #[must_use]
    pub fn https_url(&self) -> String {
        format!("https://github.com/{}/{}.git", self.owner, self.name)
    }
}

impl std::fmt::Display for RepoRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// The App's bot, who commits, pushes and opens pull requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotIdentity {
    /// The App's slug (`aura-agents`).
    pub slug: String,
    /// The bot account's GitHub id, which makes its commit email link
    /// to it; `None` when it couldn't be looked up.
    pub account_id: Option<u64>,
}

impl BotIdentity {
    /// `<slug>[bot]`, the git author name.
    #[must_use]
    pub fn login(&self) -> String {
        format!("{}[bot]", self.slug)
    }

    /// `<id>+<slug>[bot]@users.noreply.github.com`, the git author email.
    #[must_use]
    pub fn email(&self) -> String {
        match self.account_id {
            Some(id) => format!("{id}+{}[bot]@users.noreply.github.com", self.slug),
            None => format!("{}[bot]@users.noreply.github.com", self.slug),
        }
    }

    /// Where a repository owner installs the App.
    #[must_use]
    pub fn install_url(&self) -> String {
        format!("https://github.com/apps/{}/installations/new", self.slug)
    }
}

/// An installation token for one repository.
pub struct RepoToken {
    /// The token (`ghs_…`); never logged.
    pub token: Zeroizing<String>,
    /// When GitHub stops accepting it, Unix milliseconds.
    pub expires_at_ms: u64,
}

impl std::fmt::Debug for RepoToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepoToken")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish_non_exhaustive()
    }
}

/// Why a repository token could not be had.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GithubError {
    /// The App isn't installed on the repository, or the installation
    /// doesn't include it.
    #[error(
        "the GitHub App isn't installed on {repo}{}",
        install_url.as_ref().map(|url| format!(": install it at {url}")).unwrap_or_default()
    )]
    NotInstalled {
        /// `owner/name`.
        repo: String,
        /// Where to install it, when the App's slug is known.
        install_url: Option<String>,
    },
    /// The private key could not be read or used.
    #[error("GitHub App key {path}: {message}")]
    Key {
        /// The key file.
        path: String,
        /// What was wrong (never the key).
        message: String,
    },
    /// GitHub answered with an error.
    #[error("GitHub {op} failed ({status}): {message}")]
    Api {
        /// The call.
        op: &'static str,
        /// HTTP status.
        status: u16,
        /// GitHub's `message`.
        message: String,
    },
    /// GitHub could not be reached in time.
    #[error("could not reach GitHub for {op}: {message}")]
    Transport {
        /// The call.
        op: &'static str,
        /// Why.
        message: String,
    },
    /// GitHub answered with something this build can't use.
    #[error("unexpected answer from GitHub {op}: {message}")]
    InvalidResponse {
        /// The call.
        op: &'static str,
        /// What was wrong.
        message: String,
    },
}

/// Where the hub gets repository tokens. [`GithubApp`] implements it over
/// GitHub; [`FakeRepoTokens`] in memory.
#[async_trait]
pub trait RepoTokens: Send + Sync {
    /// The bot commits and pull requests are made as.
    ///
    /// # Errors
    ///
    /// When GitHub can't be asked or the key can't sign.
    async fn identity(&self) -> Result<BotIdentity, GithubError>;

    /// A token for `repo` alone.
    ///
    /// # Errors
    ///
    /// [`GithubError::NotInstalled`] when the App can't reach `repo`;
    /// otherwise as [`Self::identity`].
    async fn mint(&self, repo: &RepoRef) -> Result<RepoToken, GithubError>;
}

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
                message: err.to_string(),
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
                message: err.to_string(),
            })?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|err| GithubError::Transport {
                op,
                message: err.to_string(),
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

/// What [`FakeRepoTokens`] mints for a repo: a token and when it expires.
type FakeMint = Result<(String, u64), GithubError>;

/// [`RepoTokens`] in memory, for tests.
#[derive(Debug)]
pub struct FakeRepoTokens {
    identity: BotIdentity,
    tokens: Mutex<HashMap<RepoRef, FakeMint>>,
    mints: Mutex<Vec<RepoRef>>,
}

impl FakeRepoTokens {
    /// An App with slug `slug`, installed nowhere yet.
    #[must_use]
    pub fn new(slug: &str) -> Self {
        Self {
            identity: BotIdentity {
                slug: slug.to_string(),
                account_id: Some(41_000_000),
            },
            tokens: Mutex::new(HashMap::new()),
            mints: Mutex::new(Vec::new()),
        }
    }

    /// From now on, minting for `repo` gives `token`, expiring at
    /// `expires_at_ms`.
    pub fn install(&self, repo: &RepoRef, token: &str, expires_at_ms: u64) {
        self.tokens
            .lock()
            .insert(repo.clone(), Ok((token.to_string(), expires_at_ms)));
    }

    /// From now on, minting for `repo` fails with `err`.
    pub fn fail(&self, repo: &RepoRef, err: GithubError) {
        self.tokens.lock().insert(repo.clone(), Err(err));
    }

    /// How often a token for `repo` was asked for.
    #[must_use]
    pub fn mints(&self, repo: &RepoRef) -> usize {
        self.mints.lock().iter().filter(|r| *r == repo).count()
    }
}

#[async_trait]
impl RepoTokens for FakeRepoTokens {
    async fn identity(&self) -> Result<BotIdentity, GithubError> {
        Ok(self.identity.clone())
    }

    async fn mint(&self, repo: &RepoRef) -> Result<RepoToken, GithubError> {
        self.mints.lock().push(repo.clone());
        match self.tokens.lock().get(repo).cloned() {
            Some(Ok((token, expires_at_ms))) => Ok(RepoToken {
                token: Zeroizing::new(token),
                expires_at_ms,
            }),
            Some(Err(err)) => Err(err),
            None => Err(GithubError::NotInstalled {
                repo: repo.full_name(),
                install_url: Some(self.identity.install_url()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use jsonwebtoken::{DecodingKey, Validation};
    use std::sync::Arc;

    const PEM: &[u8] = include_bytes!("fixtures/test_rsa.pem");
    const JWKS: &str = include_str!("fixtures/test_jwks.json");

    #[test]
    fn remotes_on_github_parse_and_others_do_not() {
        let repo = |owner: &str, name: &str| {
            Some(RepoRef {
                owner: owner.into(),
                name: name.into(),
            })
        };
        for url in [
            "https://github.com/cypher-asi/aura-harness.git",
            "https://github.com/cypher-asi/aura-harness",
            "https://github.com/cypher-asi/aura-harness/",
            "https://x-access-token:ghs_secret@github.com/cypher-asi/aura-harness.git",
            "ssh://git@github.com/cypher-asi/aura-harness.git",
            "ssh://git@github.com:22/cypher-asi/aura-harness.git",
            "git@github.com:cypher-asi/aura-harness.git",
            "github.com:cypher-asi/aura-harness",
            "  git@GitHub.com:cypher-asi/aura-harness.git\n",
        ] {
            assert_eq!(
                RepoRef::from_remote(url),
                repo("cypher-asi", "aura-harness"),
                "{url}"
            );
        }
        assert_eq!(
            RepoRef::from_remote("https://github.com/a/b.c.d.git"),
            repo("a", "b.c.d")
        );
        for url in [
            "https://gitlab.com/a/b.git",
            "https://github.com.evil.example/a/b.git",
            "git@gitlab.com:a/b.git",
            "https://github.com/a",
            "https://github.com/a/b/c",
            "https://github.com/../b",
            "file:///srv/a/b.git",
            "/srv/repos/a.git",
            "",
        ] {
            assert_eq!(RepoRef::from_remote(url), None, "{url}");
        }
        let parsed = repo("o", "r").expect("repo");
        assert_eq!(parsed.full_name(), "o/r");
        assert_eq!(parsed.https_url(), "https://github.com/o/r.git");
    }

    #[test]
    fn the_bot_identity_is_the_apps_noreply_account() {
        let bot = BotIdentity {
            slug: "aura-agents".into(),
            account_id: Some(123),
        };
        assert_eq!(bot.login(), "aura-agents[bot]");
        assert_eq!(bot.email(), "123+aura-agents[bot]@users.noreply.github.com");
        assert_eq!(
            bot.install_url(),
            "https://github.com/apps/aura-agents/installations/new"
        );
    }

    #[test]
    fn tokens_never_print() {
        let token = RepoToken {
            token: Zeroizing::new("ghs_supersecret".into()),
            expires_at_ms: 1,
        };
        assert!(!format!("{token:?}").contains("ghs_"));
    }

    /// What the mock GitHub saw and serves.
    #[derive(Default)]
    struct Mock {
        installed: HashMap<String, u64>,
        jwts: Vec<String>,
        token_requests: Vec<Value>,
    }

    type Shared = Arc<parking_lot::Mutex<Mock>>;

    fn bearer(headers: &HeaderMap) -> String {
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or_default()
            .to_string()
    }

    async fn installation(
        State(mock): State<Shared>,
        Path((owner, repo)): Path<(String, String)>,
        headers: HeaderMap,
    ) -> (StatusCode, Json<Value>) {
        let mut mock = mock.lock();
        mock.jwts.push(bearer(&headers));
        match mock.installed.get(&format!("{owner}/{repo}")) {
            Some(id) => (StatusCode::OK, Json(json!({ "id": id }))),
            None => (
                StatusCode::NOT_FOUND,
                Json(json!({ "message": "Not Found" })),
            ),
        }
    }

    async fn access_token(
        State(mock): State<Shared>,
        Path(id): Path<u64>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        let mut mock = mock.lock();
        mock.jwts.push(bearer(&headers));
        mock.token_requests.push(body);
        if !mock.installed.values().any(|known| *known == id) {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "message": "Not Found" })),
            );
        }
        (
            StatusCode::CREATED,
            Json(json!({
                "token": format!("ghs_minted_{id}"),
                "expires_at": "2030-01-01T00:00:00Z",
            })),
        )
    }

    async fn app(State(mock): State<Shared>, headers: HeaderMap) -> Json<Value> {
        mock.lock().jwts.push(bearer(&headers));
        Json(json!({ "id": 7, "slug": "aura-agents", "name": "Aura agents" }))
    }

    async fn bot_account(Path(login): Path<String>) -> (StatusCode, Json<Value>) {
        if login == "aura-agents[bot]" {
            (StatusCode::OK, Json(json!({ "id": 4242, "login": login })))
        } else {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "message": "Not Found" })),
            )
        }
    }

    async fn serve(mock: Shared) -> String {
        let router = Router::new()
            .route("/repos/:owner/:repo/installation", get(installation))
            .route("/app/installations/:id/access_tokens", post(access_token))
            .route("/app", get(app))
            .route("/users/:login", get(bot_account))
            .with_state(mock);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        format!("http://{addr}")
    }

    fn app_config(dir: &std::path::Path, workflows: bool) -> GithubAppConfig {
        let key = dir.join("app.pem");
        std::fs::write(&key, PEM).expect("write key");
        GithubAppConfig {
            app_id: "Iv23liTestApp".into(),
            private_key_path: key,
            workflows,
        }
    }

    fn verify(jwt: &str) -> Value {
        let jwks: Value = serde_json::from_str(JWKS).expect("jwks");
        let jwk: jsonwebtoken::jwk::Jwk =
            serde_json::from_value(jwks["keys"][0].clone()).expect("jwk");
        // `iss` is checked by each test: jsonwebtoken counts a numeric
        // `iss` (an app id, which GitHub accepts) as missing.
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_required_spec_claims(&["exp", "iat"]);
        let claims = jsonwebtoken::decode::<Value>(
            jwt,
            &DecodingKey::from_jwk(&jwk).expect("key"),
            &validation,
        )
        .expect("the App JWT verifies with the App's public key")
        .claims;
        assert!(
            claims["iss"].is_string() || claims["iss"].is_u64(),
            "{claims}"
        );
        claims
    }

    #[tokio::test]
    async fn a_token_is_minted_for_one_repo_with_narrowed_permissions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mock: Shared = Arc::default();
        mock.lock().installed.insert("acme/app".into(), 99);
        let base = serve(Arc::clone(&mock)).await;
        let github = GithubApp::with_api(app_config(dir.path(), false), base).expect("app");
        let repo = RepoRef::from_remote("git@github.com:acme/app.git").expect("repo");

        let token = github.mint(&repo).await.expect("mint");
        assert_eq!(token.token.as_str(), "ghs_minted_99");
        assert_eq!(token.expires_at_ms, 1_893_456_000_000);
        github.mint(&repo).await.expect("mint again");

        let mock = mock.lock();
        assert_eq!(
            mock.token_requests[0],
            json!({
                "repositories": ["app"],
                "permissions": {"contents": "write", "pull_requests": "write"},
            })
        );
        // The installation was looked up once, then cached: one lookup,
        // two mints, each with a JWT of its own.
        assert_eq!(mock.jwts.len(), 3);
        let claims = verify(&mock.jwts[0]);
        assert_eq!(claims["iss"], "Iv23liTestApp");
        let now = chrono::Utc::now().timestamp();
        let iat = claims["iat"].as_i64().expect("iat");
        let exp = claims["exp"].as_i64().expect("exp");
        assert!((now - 70..=now - 50).contains(&iat), "iat {iat} now {now}");
        assert_eq!(exp - iat, JWT_BACKDATE_SECS + JWT_LIFETIME_SECS);
        assert!(exp - now <= 600, "GitHub refuses JWTs over ten minutes");
    }

    #[tokio::test]
    async fn workflows_are_granted_only_when_configured_and_app_ids_are_numbers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mock: Shared = Arc::default();
        mock.lock().installed.insert("acme/app".into(), 5);
        let base = serve(Arc::clone(&mock)).await;
        let mut config = app_config(dir.path(), true);
        config.app_id = "12345".into();
        let github = GithubApp::with_api(config, base).expect("app");
        let repo = RepoRef::from_remote("https://github.com/acme/app").expect("repo");
        github.mint(&repo).await.expect("mint");
        let mock = mock.lock();
        assert_eq!(
            mock.token_requests[0]["permissions"],
            json!({"contents": "write", "pull_requests": "write", "workflows": "write"})
        );
        assert_eq!(verify(&mock.jwts[0])["iss"], json!(12345));
    }

    #[tokio::test]
    async fn a_repo_without_the_app_gets_the_install_link() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mock: Shared = Arc::default();
        let base = serve(Arc::clone(&mock)).await;
        let github = GithubApp::with_api(app_config(dir.path(), false), base).expect("app");
        let repo = RepoRef::from_remote("https://github.com/acme/private").expect("repo");
        let err = github.mint(&repo).await.expect_err("not installed");
        assert_eq!(
            err,
            GithubError::NotInstalled {
                repo: "acme/private".into(),
                install_url: Some("https://github.com/apps/aura-agents/installations/new".into()),
            }
        );
        assert!(err.to_string().contains("installations/new"), "{err}");
        assert!(mock.lock().token_requests.is_empty());

        let bot = github.identity().await.expect("identity");
        assert_eq!(bot.login(), "aura-agents[bot]");
        assert_eq!(
            bot.email(),
            "4242+aura-agents[bot]@users.noreply.github.com"
        );
    }

    #[tokio::test]
    async fn an_unreadable_key_is_a_key_error_that_never_holds_the_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = app_config(dir.path(), false);
        std::fs::write(&config.private_key_path, b"not a pem").expect("write");
        let github = GithubApp::with_api(config.clone(), "http://127.0.0.1:9").expect("app");
        let repo = RepoRef::from_remote("https://github.com/acme/app").expect("repo");
        let err = github.mint(&repo).await.expect_err("bad key");
        assert!(matches!(err, GithubError::Key { .. }), "{err}");
        config.private_key_path = dir.path().join("missing.pem");
        let github = GithubApp::with_api(config, "http://127.0.0.1:9").expect("app");
        let err = github.mint(&repo).await.expect_err("missing key");
        assert!(err.to_string().contains("could not read"), "{err}");
        assert!(!format!("{github:?}").contains("PRIVATE"));
    }
}
