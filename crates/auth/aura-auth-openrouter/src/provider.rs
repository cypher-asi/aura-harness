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
    LinkGrant {
        tokens: OAuthTokens {
            access_token: key,
            refresh_token: None,
            id_token: None,
            scope: String::new(),
            expires_at: Utc::now() + Duration::days(KEY_LIFETIME_DAYS),
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
mod tests {
    use super::*;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    type Seen = Arc<Mutex<Vec<Value>>>;

    const KEY: &str = "sk-or-v1-secret-test-key";

    async fn serve() -> (OpenRouterProvider, Seen) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen: Seen = Arc::default();
        let log = seen.clone();
        let app = Router::new()
            .route(
                "/api/v1/auth/keys",
                post(move |Json(body): Json<Value>| {
                    let log = log.clone();
                    async move {
                        log.lock().unwrap().push(body.clone());
                        if body["code"] == "expired-code" {
                            return (
                                axum::http::StatusCode::FORBIDDEN,
                                Json(json!({"error": {"code": 403, "message": "Invalid code"}})),
                            );
                        }
                        (
                            axum::http::StatusCode::OK,
                            Json(json!({"key": KEY, "user_id": "user-1"})),
                        )
                    }
                }),
            )
            .route(
                "/api/v1/key",
                get(|headers: axum::http::HeaderMap| async move {
                    if headers["authorization"] != format!("Bearer {KEY}").as_str() {
                        return (
                            axum::http::StatusCode::UNAUTHORIZED,
                            Json(json!({"error": {"code": 401, "message": "No auth"}})),
                        );
                    }
                    (
                        axum::http::StatusCode::OK,
                        Json(json!({"data": {
                            "label": "Aura key",
                            "limit": 10.0,
                            "limit_remaining": 4.5,
                            "usage": 5.5,
                            "is_free_tier": false,
                        }})),
                    )
                }),
            );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (OpenRouterProvider::fixture(&base).unwrap(), seen)
    }

    fn query(provider: &OpenRouterProvider, redirect: &str) -> (url::Url, HashMap<String, String>) {
        let request = aura_auth_kit::new_attempt(CLIENT_ID.into(), redirect.into(), None);
        let url = url::Url::parse(&provider.authorization_url(&request).unwrap()).unwrap();
        let pairs = url.query_pairs().into_owned().collect();
        (url, pairs)
    }

    fn linked() -> LinkedAccount {
        LinkedAccount {
            id: "l".into(),
            provider: ProviderKind::OpenRouter,
            owner: aura_core_auth::Owner::Local,
            subject: "user-1".into(),
            label: "Aura key".into(),
            client_id: CLIENT_ID.into(),
            plan_enabled: true,
            expires_at: Utc::now(),
            created_at: Utc::now(),
            status: aura_core_auth::LinkStatus::Active,
            plan: None,
        }
    }

    fn tokens(access_token: &str) -> OAuthTokens {
        OAuthTokens {
            access_token: access_token.into(),
            refresh_token: None,
            id_token: None,
            scope: String::new(),
            expires_at: Utc::now(),
        }
    }

    #[test]
    fn authorization_url_with_callback() {
        let provider = OpenRouterProvider::production();
        let request = aura_auth_kit::new_attempt(
            CLIENT_ID.into(),
            "http://localhost:4321/callback".into(),
            None,
        );
        let url = url::Url::parse(&provider.authorization_url(&request).unwrap()).unwrap();
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(url.host_str(), Some("openrouter.ai"));
        assert_eq!(url.path(), "/auth");
        assert_eq!(query["callback_url"], "http://localhost:4321/callback");
        assert_eq!(query["state"], request.state);
        assert_eq!(query["code_challenge"], request.code_challenge);
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(query["key_label"], KEY_LABEL);
    }

    #[test]
    fn authorization_url_headless_omits_callback_and_state() {
        let (_, query) = query(&OpenRouterProvider::production(), MANUAL_REDIRECT);
        assert!(!query.contains_key("callback_url"), "{query:?}");
        assert!(!query.contains_key("state"), "{query:?}");
        assert_eq!(query["code_challenge_method"], "S256");
        assert!(!query["code_challenge"].is_empty());
    }

    #[test]
    fn api_base_override_derives_endpoints() {
        let p = OpenRouterProvider::for_api_base("http://127.0.0.1:9/proxy/api/").unwrap();
        assert_eq!(p.authorize_url, "http://127.0.0.1:9/proxy/auth");
        assert_eq!(p.keys_url, "http://127.0.0.1:9/proxy/api/v1/auth/keys");
        assert_eq!(p.key_info_url, "http://127.0.0.1:9/proxy/api/v1/key");
        let p = OpenRouterProvider::for_api_base(API_BASE).unwrap();
        assert_eq!(p.authorize_url, AUTHORIZE_URL);
        assert_eq!(p.keys_url, KEYS_URL);
        assert_eq!(p.key_info_url, KEY_INFO_URL);
        for unsafe_base in [
            "http://example.com/api",
            "not a url",
            "https://x.test/api?a=1",
        ] {
            assert!(
                OpenRouterProvider::for_api_base(unsafe_base).is_none(),
                "{unsafe_base}"
            );
        }
    }

    #[test]
    fn fixture_requires_numeric_loopback_http() {
        assert!(OpenRouterProvider::fixture("http://127.0.0.1:1").is_ok());
        for base in [
            "https://127.0.0.1:1",
            "http://localhost:1",
            "http://example.com",
        ] {
            assert!(OpenRouterProvider::fixture(base).is_err(), "{base}");
        }
    }

    #[test]
    fn key_info_tiers() {
        let tier = |v: Value| serde_json::from_value::<KeyInfo>(v).unwrap().tier();
        assert_eq!(
            tier(json!({"is_free_tier": true, "limit_remaining": 1.0})),
            "free"
        );
        assert_eq!(tier(json!({"limit_remaining": 4.5})), "$4.50 left");
        assert_eq!(tier(json!({"limit_remaining": null})), "credits");
    }

    #[test]
    fn credential_carries_attribution_headers() {
        let credential = OpenRouterProvider::production().credential(&tokens(KEY));
        assert_eq!(credential.provider, ProviderKind::OpenRouter);
        assert_eq!(credential.bearer, KEY);
        assert_eq!(
            credential.headers,
            vec![
                ("HTTP-Referer".to_string(), APP_REFERER.to_string()),
                ("X-Title".to_string(), APP_TITLE.to_string()),
            ]
        );
        assert_eq!(credential.system_preamble, None);
    }

    #[test]
    fn recognises_pasted_keys() {
        let provider = OpenRouterProvider::production();
        assert!(provider.is_api_key("  sk-or-v1-abc\n"));
        assert!(!provider.is_api_key("abc123"));
        assert!(!provider.is_api_key("sk-ant-oat-1"));
    }

    #[tokio::test]
    async fn exchange_yields_key_without_refresh() {
        let (provider, seen) = serve().await;
        let request = aura_auth_kit::new_attempt(CLIENT_ID.into(), MANUAL_REDIRECT.into(), None);
        let callback = aura_auth_kit::parse_pasted_code("the-code", &request.state).unwrap();
        let grant = provider.exchange(&request, &callback).await.unwrap();

        let sent = seen.lock().unwrap()[0].clone();
        assert_eq!(sent["code"], "the-code");
        assert_eq!(sent["code_verifier"], request.code_verifier.as_str());
        assert_eq!(sent["code_challenge_method"], "S256");

        assert_eq!(grant.tokens.access_token, KEY);
        assert_eq!(grant.tokens.refresh_token, None);
        assert!(grant.tokens.expires_at > Utc::now() + Duration::days(3000));
        assert_eq!(grant.subject.as_deref(), Some("user-1"));
        assert_eq!(grant.label.as_deref(), Some("Aura key"));
        assert_eq!(grant.client_id, CLIENT_ID);
        assert!(grant.plan_enabled);

        let rendered = format!("{:?}", grant.tokens);
        assert!(!rendered.contains(KEY), "{rendered}");
        assert!(!format!("{grant:?}").contains(KEY));

        assert_eq!(
            provider.plan_tier(&grant.tokens).await.as_deref(),
            Some("$4.50 left")
        );
        assert_eq!(provider.plan_tier(&tokens("sk-or-v1-revoked")).await, None);
    }

    #[tokio::test]
    async fn exchange_rejected_code() {
        let (provider, _) = serve().await;
        let request = aura_auth_kit::new_attempt(CLIENT_ID.into(), MANUAL_REDIRECT.into(), None);
        let callback = aura_auth_kit::parse_pasted_code("expired-code", &request.state).unwrap();
        let err = provider.exchange(&request, &callback).await.unwrap_err();
        assert!(
            matches!(
                &err,
                AuthError::Rejected { status: 403, code, .. } if code == "code_exchange_failed"
            ),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn exchange_validates_callback_before_network() {
        let provider = OpenRouterProvider::production();
        let request = aura_auth_kit::new_attempt(CLIENT_ID.into(), MANUAL_REDIRECT.into(), None);
        let declined = CallbackParams {
            error: Some("access_denied".into()),
            ..CallbackParams::default()
        };
        let err = provider.exchange(&request, &declined).await.unwrap_err();
        assert!(matches!(err, AuthError::Declined(_)), "{err:?}");
        let oversized = CallbackParams {
            code: Some("x".repeat(MAX_SECRET_LEN + 1)),
            ..CallbackParams::default()
        };
        for callback in [CallbackParams::default(), oversized] {
            let err = provider.exchange(&request, &callback).await.unwrap_err();
            assert!(matches!(err, AuthError::Invalid(_)), "{err:?}");
        }
    }

    #[tokio::test]
    async fn accept_api_key_checks_key() {
        let (provider, _) = serve().await;
        let grant = provider
            .accept_api_key(&format!("  {KEY}\n"))
            .await
            .unwrap();
        assert_eq!(grant.tokens.access_token, KEY);
        assert_eq!(grant.tokens.refresh_token, None);
        assert_eq!(grant.label.as_deref(), Some("Aura key"));
        assert_eq!(grant.client_id, CLIENT_ID);
        let subject = grant.subject.unwrap();
        assert_eq!(subject.len(), 16);
        assert!(subject.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(!KEY.contains(&subject));

        let err = provider
            .accept_api_key("sk-or-v1-revoked")
            .await
            .unwrap_err();
        assert!(
            matches!(
                &err,
                AuthError::Rejected { status: 401, code, .. } if code == "invalid_key"
            ),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn accept_api_key_rejects_non_keys_before_network() {
        let provider = OpenRouterProvider::production();
        for pasted in ["the-code", "sk-ant-oat-1", "sk-or-has space", ""] {
            let err = provider.accept_api_key(pasted).await.unwrap_err();
            assert!(matches!(err, AuthError::Invalid(_)), "{pasted}: {err:?}");
        }
    }

    #[tokio::test]
    async fn refresh_requires_new_sign_in() {
        let err = OpenRouterProvider::production()
            .refresh(&linked(), &tokens(KEY))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::ReauthRequired(_)), "{err:?}");
    }
}
