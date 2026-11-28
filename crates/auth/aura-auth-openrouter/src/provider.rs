//! [`SubscriptionProvider`] implementation for OpenRouter.

use crate::{
    API_BASE, API_URL_ENV, APP_REFERER, APP_TITLE, AUTHORIZE_URL, CLIENT_ID, KEYS_URL,
    KEY_INFO_URL, KEY_LABEL, KEY_PREFIX, MANUAL_REDIRECT,
};
use async_trait::async_trait;
use aura_auth_kit::http;
use aura_core_auth::{
    AuthError, AuthorizationRequest, CallbackParams, InferenceCredential, LinkGrant, LinkedAccount,
    LoopbackRedirect, OAuthTokens, ProviderKind, RedirectStrategy, SubscriptionProvider,
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

const NAME: &str = "OpenRouter";

const REDIRECT: LoopbackRedirect = LoopbackRedirect {
    host: "localhost",
    path: "/callback",
};

/// Keys do not expire on their own; this keeps the manager from
/// treating a stored key as due for refresh.
const KEY_LIFETIME_DAYS: i64 = 3650;

/// Longest authorization code or API key accepted from outside.
const MAX_SECRET_LEN: usize = 8192;

#[derive(Deserialize)]
struct KeyResponse {
    #[serde(default)]
    key: String,
    #[serde(default)]
    user_id: Option<String>,
}

#[derive(Deserialize)]
struct KeyInfoResponse {
    data: KeyInfo,
}

#[derive(Default, Deserialize)]
struct KeyInfo {
    #[serde(default)]
    label: String,
    #[serde(default)]
    limit_remaining: Option<f64>,
    #[serde(default)]
    is_free_tier: bool,
}

impl KeyInfo {
    /// `free` for free-tier accounts, the remaining spend limit when the
    /// key has one, and `credits` (pay as you go) otherwise.
    fn tier(&self) -> String {
        if self.is_free_tier {
            return "free".into();
        }
        match self.limit_remaining {
            Some(remaining) => format!("${remaining:.2} left"),
            None => "credits".into(),
        }
    }
}

/// OpenRouter sign-in: OAuth PKCE or a pasted `sk-or-` key.
pub struct OpenRouterProvider {
    authorize_url: String,
    keys_url: String,
    key_info_url: String,
}

impl Default for OpenRouterProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenRouterProvider {
    /// Production provider, honouring [`API_URL_ENV`] (an API base such
    /// as `https://openrouter.ai/api`; the sign-in page is derived by
    /// dropping the trailing `/api`).
    ///
    /// An override that is not HTTPS (or HTTP on a loopback host) is
    /// ignored and the production endpoints are used, so a bad value can
    /// never send keys over plain HTTP to a remote host.
    #[must_use]
    pub fn new() -> Self {
        std::env::var(API_URL_ENV)
            .ok()
            .and_then(|base| Self::for_api_base(&base))
            .unwrap_or_else(Self::production)
    }

    fn production() -> Self {
        Self {
            authorize_url: AUTHORIZE_URL.into(),
            keys_url: KEYS_URL.into(),
            key_info_url: KEY_INFO_URL.into(),
        }
    }

    /// Endpoints under an API base, or `None` when the base is unsafe.
    fn for_api_base(api_base: &str) -> Option<Self> {
        let api_base = api_base.trim().trim_end_matches('/');
        let url = url::Url::parse(api_base).ok()?;
        let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        let secure = url.scheme() == "https" || (url.scheme() == "http" && loopback);
        if !secure || url.query().is_some() || url.fragment().is_some() {
            return None;
        }
        if api_base == API_BASE {
            return Some(Self::production());
        }
        let site = api_base.strip_suffix("/api").unwrap_or(api_base);
        Some(Self {
            authorize_url: format!("{site}/auth"),
            keys_url: format!("{api_base}/v1/auth/keys"),
            key_info_url: format!("{api_base}/v1/key"),
        })
    }

    /// Provider pointed at a credential-free numeric-loopback test server
    /// serving `/auth`, `/api/v1/auth/keys` and `/api/v1/key`.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::Invalid`] for any other origin.
    pub fn fixture(base: &str) -> Result<Self, AuthError> {
        let url =
            url::Url::parse(base).map_err(|_| AuthError::Invalid("Invalid fixture URL".into()))?;
        if url.scheme() != "http" || url.host_str() != Some("127.0.0.1") {
            return Err(AuthError::Invalid(
                "Fixture endpoints must use numeric loopback HTTP".into(),
            ));
        }
        let base = base.trim_end_matches('/');
        Ok(Self {
            authorize_url: format!("{base}/auth"),
            keys_url: format!("{base}/api/v1/auth/keys"),
            key_info_url: format!("{base}/api/v1/key"),
        })
    }

    async fn key_info_request(&self, key: &str) -> Result<reqwest::Response, AuthError> {
        http::client()?
            .get(&self.key_info_url)
            .bearer_auth(key)
            .send()
            .await
            .map_err(|e| http::unavailable(NAME, &e))
    }

    /// Best-effort key details; `None` on any failure.
    async fn key_info(&self, key: &str) -> Option<KeyInfo> {
        let response = self.key_info_request(key).await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        response
            .json::<KeyInfoResponse>()
            .await
            .ok()
            .map(|r| r.data)
    }
}

/// Whether a failed status is worth retrying rather than restarting sign-in.
fn is_transient(status: reqwest::StatusCode) -> bool {
    status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

fn temporarily_unavailable() -> AuthError {
    AuthError::Unavailable(format!(
        "{NAME} sign-in service is temporarily unavailable; retry later"
    ))
}

/// Stable, non-secret account id derived from a key: the first 16 hex
/// characters of its SHA-256 digest.
fn key_subject(key: &str) -> String {
    Sha256::digest(key.as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Link grant for an OpenRouter API key, from sign-in or pasted.
fn grant_for_key(key: String, user_id: Option<String>, info: Option<&KeyInfo>) -> LinkGrant {
    let subject = user_id
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| key_subject(&key));
    let now = Utc::now();
    LinkGrant {
        tokens: OAuthTokens {
            access_token: key,
            refresh_token: None,
            id_token: None,
            scope: String::new(),
            expires_at: now + Duration::days(KEY_LIFETIME_DAYS),
            issued_at: Some(now),
        },
        subject: Some(subject),
        label: info.map(|i| i.label.clone()).filter(|l| !l.is_empty()),
        client_id: CLIENT_ID.into(),
        plan_enabled: true,
    }
}

#[async_trait]
impl SubscriptionProvider for OpenRouterProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenRouter
    }

    fn redirect(&self) -> RedirectStrategy {
        RedirectStrategy {
            loopback: Some(REDIRECT),
            manual: Some(MANUAL_REDIRECT),
        }
    }

    fn client_id(&self, _previous: Option<&str>) -> String {
        CLIENT_ID.to_string()
    }

    fn authorization_url(&self, request: &AuthorizationRequest) -> Result<String, AuthError> {
        let mut url = url::Url::parse(&self.authorize_url)
            .map_err(|_| AuthError::Invalid("Invalid OpenRouter authority".into()))?;
        {
            let mut query = url.query_pairs_mut();
            // Headless: without `callback_url` the page shows the code and
            // returns no state, so neither is sent.
            if request.redirect_uri != MANUAL_REDIRECT {
                query.append_pair("callback_url", &request.redirect_uri);
            }
            query.extend_pairs([
                ("code_challenge", request.code_challenge.as_str()),
                ("code_challenge_method", "S256"),
            ]);
            if request.redirect_uri != MANUAL_REDIRECT {
                query.append_pair("state", &request.state);
            }
            query.append_pair("key_label", KEY_LABEL);
        }
        Ok(url.into())
    }

    async fn exchange(
        &self,
        request: &AuthorizationRequest,
        callback: &CallbackParams,
    ) -> Result<LinkGrant, AuthError> {
        if callback.error.is_some() {
            return Err(AuthError::Declined(
                "OpenRouter sign-in was declined".into(),
            ));
        }
        let code = callback
            .code
            .as_deref()
            .filter(|c| !c.is_empty() && c.len() <= MAX_SECRET_LEN)
            .ok_or_else(|| AuthError::Invalid("Missing authorization code".into()))?;
        let response = http::client()?
            .post(&self.keys_url)
            .json(&json!({
                "code": code,
                "code_verifier": request.code_verifier,
                "code_challenge_method": "S256",
            }))
            .send()
            .await
            .map_err(|e| http::unavailable(NAME, &e))?;
        let status = response.status();
        if is_transient(status) {
            return Err(temporarily_unavailable());
        }
        if !status.is_success() {
            return Err(AuthError::Rejected {
                status: status.as_u16(),
                code: "code_exchange_failed".into(),
                message: "OpenRouter code exchange failed; start sign-in again".into(),
            });
        }
        let body: KeyResponse = http::json(NAME, response).await?;
        if body.key.is_empty() {
            return Err(AuthError::Malformed(
                "OpenRouter returned no API key".into(),
            ));
        }
        let info = self.key_info(&body.key).await;
        Ok(grant_for_key(body.key, body.user_id, info.as_ref()))
    }

    async fn refresh(
        &self,
        _account: &LinkedAccount,
        _tokens: &OAuthTokens,
    ) -> Result<LinkGrant, AuthError> {
        Err(AuthError::ReauthRequired(
            "OpenRouter keys do not renew; run `aura login openrouter` again".into(),
        ))
    }

    async fn plan_tier(&self, tokens: &OAuthTokens) -> Option<String> {
        self.key_info(&tokens.access_token)
            .await
            .map(|info| info.tier())
    }

    fn credential(&self, tokens: &OAuthTokens) -> InferenceCredential {
        InferenceCredential {
            provider: ProviderKind::OpenRouter,
            bearer: tokens.access_token.clone(),
            headers: vec![
                ("HTTP-Referer".into(), APP_REFERER.into()),
                ("X-Title".into(), APP_TITLE.into()),
            ],
            system_preamble: None,
        }
    }

    async fn accept_api_key(&self, key: &str) -> Result<LinkGrant, AuthError> {
        let key = key.trim();
        let well_formed = self.is_api_key(key)
            && key.len() <= MAX_SECRET_LEN
            && key.bytes().all(|b| b.is_ascii_graphic());
        if !well_formed {
            return Err(AuthError::Invalid(format!(
                "That is not an OpenRouter API key (they start with `{KEY_PREFIX}`)"
            )));
        }
        let response = self.key_info_request(key).await?;
        let status = response.status();
        if matches!(status.as_u16(), 401 | 403) {
            return Err(AuthError::Rejected {
                status: status.as_u16(),
                code: "invalid_key".into(),
                message: "OpenRouter rejected that API key".into(),
            });
        }
        if !status.is_success() {
            return Err(temporarily_unavailable());
        }
        let body: KeyInfoResponse = http::json(NAME, response).await?;
        Ok(grant_for_key(key.to_string(), None, Some(&body.data)))
    }

    fn is_api_key(&self, pasted: &str) -> bool {
        pasted.trim().starts_with(KEY_PREFIX)
    }
}

#[cfg(test)]
mod tests;
