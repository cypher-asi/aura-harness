use super::*;

use crate::vault::Vault;

use async_trait::async_trait;

use aura_core_auth::{
    AuthError as ProviderError, AuthorizationRequest, CredentialResolver, IdentityProvider,
    InferenceCredential, LinkGrant, LinkedAccount, OAuthTokens, Owner, ProviderKind, StoredSession,
    SubscriptionProvider,
};

use aura_core_auth::{CallbackParams, LoopbackRedirect, RedirectStrategy};

use chrono::Duration;

use chrono::Utc;

use std::sync::atomic::{AtomicUsize, Ordering};

use std::sync::{Arc, Mutex};

struct FakeZos;

#[async_trait]
impl IdentityProvider for FakeZos {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Zos
    }

    async fn login(&self, email: &str, _password: &str) -> Result<StoredSession, ProviderError> {
        Ok(StoredSession {
            access_token: format!("jwt-{email}"),
            user_id: format!("user-{email}"),
            display_name: email.to_string(),
            primary_zid: format!("0://{email}"),
            created_at: Utc::now(),
        })
    }

    async fn logout(&self, _session: &StoredSession) {}
}

#[derive(Default)]
struct FakeSub {
    loopback: bool,
    refreshes: AtomicUsize,
    expires_in: i64,
    /// How long before sign-in its tokens were issued.
    issued_ago: i64,
    refresh_outcome: Mutex<Option<ProviderError>>,
    tier: Mutex<Option<String>>,
}

fn grant(access: &str, expires_in: i64, issued_ago: i64) -> LinkGrant {
    let now = Utc::now();
    LinkGrant {
        tokens: OAuthTokens {
            access_token: access.into(),
            refresh_token: Some(format!("refresh-for-{access}")),
            id_token: None,
            scope: "user:inference".into(),
            expires_at: now + Duration::seconds(expires_in),
            issued_at: Some(now - Duration::seconds(issued_ago)),
        },
        subject: Some("subject-1".into()),
        label: Some("me@example.com".into()),
        client_id: "issued".into(),
        plan_enabled: true,
    }
}

#[async_trait]
impl SubscriptionProvider for FakeSub {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Claude
    }

    fn redirect(&self) -> RedirectStrategy {
        RedirectStrategy {
            loopback: self.loopback.then_some(LoopbackRedirect {
                host: "127.0.0.1",
                path: "/callback",
            }),
            manual: Some("https://example.test/code"),
        }
    }

    fn client_id(&self, previous: Option<&str>) -> String {
        previous.unwrap_or("dynamic").to_string()
    }

    fn authorization_url(&self, request: &AuthorizationRequest) -> Result<String, ProviderError> {
        Ok(format!(
            "https://example.test/authorize?state={}&redirect_uri={}",
            request.state, request.redirect_uri
        ))
    }

    async fn exchange(
        &self,
        _request: &AuthorizationRequest,
        callback: &CallbackParams,
    ) -> Result<LinkGrant, ProviderError> {
        match callback.code.as_deref() {
            Some("good") => Ok(grant("access-0", self.expires_in, self.issued_ago)),
            _ => Err(ProviderError::Rejected {
                status: 400,
                code: "bad".into(),
                message: "bad code".into(),
            }),
        }
    }

    async fn refresh(
        &self,
        _account: &LinkedAccount,
        _tokens: &OAuthTokens,
    ) -> Result<LinkGrant, ProviderError> {
        let n = self.refreshes.fetch_add(1, Ordering::SeqCst) + 1;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        if let Some(e) = self.refresh_outcome.lock().unwrap().take() {
            return Err(e);
        }
        Ok(grant(&format!("access-{n}"), 3600, 0))
    }

    async fn plan_tier(&self, _tokens: &OAuthTokens) -> Option<String> {
        self.tier.lock().unwrap().clone()
    }

    fn credential(&self, tokens: &OAuthTokens) -> InferenceCredential {
        InferenceCredential {
            provider: ProviderKind::Claude,
            bearer: tokens.access_token.clone(),
            headers: vec![],
            system_preamble: None,
        }
    }
}

fn manager(dir: &std::path::Path, sub: FakeSub) -> (AuthManager, Arc<FakeSub>) {
    let sub = Arc::new(sub);
    let manager = AuthManager::with_identity(Vault::files_at(dir), Arc::new(FakeZos))
        .with_subscription(sub.clone());
    (manager, sub)
}

async fn link_by_paste(manager: &AuthManager) -> LinkedAccount {
    let pending = manager
        .begin_link(ProviderKind::Claude, LinkMode::Paste)
        .await
        .unwrap();
    assert!(pending.needs_paste());
    let state = url_state(pending.url());
    manager
        .finish_link(pending, Some(&format!("good#{state}")))
        .await
        .unwrap()
}

fn url_state(url: &str) -> String {
    url.split("state=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_string()
}

// ---- OpenRouter ----------------------------------------------------------

use aura_auth_openrouter::OpenRouterProvider;

/// Test-only key; the fixture accepts nothing else.
const OPENROUTER_KEY: &str = "sk-or-test-manager-key";

/// Bodies of `POST /api/v1/auth/keys` code exchanges the fixture saw.
type Exchanges = Arc<Mutex<Vec<serde_json::Value>>>;

/// Loopback OpenRouter fixture: exchanges any code for
/// [`OPENROUTER_KEY`] and describes that key on `GET /api/v1/key`.
async fn openrouter_fixture() -> (String, Exchanges) {
    use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::{json, Value};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let exchanges = Exchanges::default();
    let log = exchanges.clone();
    let app = Router::new()
        .route(
            "/api/v1/auth/keys",
            post(move |Json(body): Json<Value>| {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push(body);
                    Json(json!({"key": OPENROUTER_KEY, "user_id": "user-or"}))
                }
            }),
        )
        .route(
            "/api/v1/key",
            get(|headers: HeaderMap| async move {
                let bearer = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok());
                if bearer != Some(format!("Bearer {OPENROUTER_KEY}").as_str()) {
                    return (StatusCode::UNAUTHORIZED, Json(json!({"error": "no"})));
                }
                (
                    StatusCode::OK,
                    Json(json!({"data": {"label": "Aura key", "limit_remaining": null}})),
                )
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, exchanges)
}

fn openrouter_manager(dir: &std::path::Path, base: &str) -> AuthManager {
    AuthManager::with_identity(Vault::files_at(dir), Arc::new(FakeZos))
        .with_subscription(Arc::new(OpenRouterProvider::fixture(base).unwrap()))
}

/// [`OpenRouterProvider`] that counts refresh calls.
struct CountingOpenRouter {
    inner: OpenRouterProvider,
    refreshes: AtomicUsize,
}

#[async_trait]
impl SubscriptionProvider for CountingOpenRouter {
    fn kind(&self) -> ProviderKind {
        self.inner.kind()
    }

    fn redirect(&self) -> RedirectStrategy {
        self.inner.redirect()
    }

    fn client_id(&self, previous: Option<&str>) -> String {
        self.inner.client_id(previous)
    }

    fn authorization_url(&self, request: &AuthorizationRequest) -> Result<String, ProviderError> {
        self.inner.authorization_url(request)
    }

    async fn exchange(
        &self,
        request: &AuthorizationRequest,
        callback: &CallbackParams,
    ) -> Result<LinkGrant, ProviderError> {
        self.inner.exchange(request, callback).await
    }

    async fn refresh(
        &self,
        account: &LinkedAccount,
        tokens: &OAuthTokens,
    ) -> Result<LinkGrant, ProviderError> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        self.inner.refresh(account, tokens).await
    }

    async fn plan_tier(&self, tokens: &OAuthTokens) -> Option<String> {
        self.inner.plan_tier(tokens).await
    }

    fn credential(&self, tokens: &OAuthTokens) -> InferenceCredential {
        self.inner.credential(tokens)
    }

    async fn accept_api_key(&self, key: &str) -> Result<LinkGrant, ProviderError> {
        self.inner.accept_api_key(key).await
    }

    fn is_api_key(&self, pasted: &str) -> bool {
        self.inner.is_api_key(pasted)
    }
}

mod linking;
mod margins;
mod openrouter;
mod refresh;
