use super::*;
use aura_core_auth::{CallbackParams, LinkGrant, LoopbackRedirect, OAuthTokens, RedirectStrategy};
use chrono::Duration;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    refresh_outcome: Mutex<Option<ProviderError>>,
    tier: Mutex<Option<String>>,
}

fn grant(access: &str, expires_in: i64) -> LinkGrant {
    LinkGrant {
        tokens: OAuthTokens {
            access_token: access.into(),
            refresh_token: Some(format!("refresh-for-{access}")),
            id_token: None,
            scope: "user:inference".into(),
            expires_at: Utc::now() + Duration::seconds(expires_in),
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
            Some("good") => Ok(grant("access-0", self.expires_in)),
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
        Ok(grant(&format!("access-{n}"), 3600))
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

#[tokio::test]
async fn paste_link_is_owned_by_local_identity() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, _) = manager(
        dir.path(),
        FakeSub {
            expires_in: 3600,
            ..FakeSub::default()
        },
    );
    let link = link_by_paste(&manager).await;
    assert_eq!(link.owner, Owner::Local);
    assert_eq!(link.label, "me@example.com");
    assert!(manager.summary().can_use(ProviderKind::Claude));
    assert!(!manager.summary().can_use(ProviderKind::Zos));
    let credential = manager
        .inference_credential(ProviderKind::Claude)
        .await
        .unwrap();
    assert_eq!(credential.bearer, "access-0");
}

#[tokio::test]
async fn browser_link_uses_loopback_callback() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, _) = manager(
        dir.path(),
        FakeSub {
            loopback: true,
            expires_in: 3600,
            ..FakeSub::default()
        },
    );
    let pending = manager
        .begin_link(ProviderKind::Claude, LinkMode::Browser)
        .await
        .unwrap();
    assert!(!pending.needs_paste());
    let redirect = pending
        .url()
        .split("redirect_uri=")
        .nth(1)
        .unwrap()
        .to_string();
    assert!(redirect.starts_with("http://127.0.0.1:"));
    let callback = format!("{redirect}?state={}&code=good", url_state(pending.url()));
    let (link, status) = tokio::join!(manager.finish_link(pending, None), async {
        reqwest::get(&callback).await.unwrap().status().as_u16()
    });
    assert_eq!(status, 200);
    assert_eq!(link.unwrap().provider, ProviderKind::Claude);
}

#[tokio::test]
async fn zos_login_scopes_links_and_move_reassigns() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, _) = manager(
        dir.path(),
        FakeSub {
            expires_in: 3600,
            ..FakeSub::default()
        },
    );
    link_by_paste(&manager).await;

    manager.login_zos("alice", "pw").await.unwrap();
    assert!(manager.links().is_empty(), "local links hidden under zOS");
    assert!(matches!(
        manager.inference_credential(ProviderKind::Claude).await,
        Err(ProviderError::NotLinked(ProviderKind::Claude))
    ));
    assert_eq!(manager.move_local_links_to_zos().unwrap(), 1);
    assert_eq!(
        manager.link(ProviderKind::Claude).unwrap().owner,
        Owner::Zos {
            user_id: "user-alice".into()
        }
    );

    manager.logout_zos().await.unwrap();
    assert!(manager.links().is_empty(), "zOS links hidden after logout");
    manager.login_zos("alice", "pw").await.unwrap();
    assert_eq!(manager.links().len(), 1, "same user gets links back");
    manager.logout_zos().await.unwrap();
    manager.login_zos("bob", "pw").await.unwrap();
    assert!(manager.links().is_empty(), "other user never sees them");
}

#[tokio::test]
async fn concurrent_requests_refresh_once() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, sub) = manager(
        dir.path(),
        FakeSub {
            expires_in: 10,
            ..FakeSub::default()
        },
    );
    link_by_paste(&manager).await;
    let (a, b, c) = tokio::join!(
        manager.inference_credential(ProviderKind::Claude),
        manager.inference_credential(ProviderKind::Claude),
        manager.inference_credential(ProviderKind::Claude),
    );
    assert_eq!(sub.refreshes.load(Ordering::SeqCst), 1);
    for credential in [a, b, c] {
        assert_eq!(credential.unwrap().bearer, "access-1");
    }
    let link = manager.link(ProviderKind::Claude).unwrap();
    assert_eq!(
        manager
            .vault()
            .tokens(&link)
            .unwrap()
            .refresh_token
            .as_deref(),
        Some("refresh-for-access-1"),
        "rotated refresh token persisted"
    );
}

#[tokio::test]
async fn requests_use_kept_tokens_without_reading_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, _) = manager(
        dir.path(),
        FakeSub {
            expires_in: 3600,
            ..FakeSub::default()
        },
    );
    let link = link_by_paste(&manager).await;
    std::fs::remove_file(dir.path().join(format!("auth/secrets/{}.json", link.id))).unwrap();
    for _ in 0..3 {
        let credential = manager
            .inference_credential(ProviderKind::Claude)
            .await
            .unwrap();
        assert_eq!(credential.bearer, "access-0");
    }
}

#[tokio::test]
async fn a_refresh_by_another_process_is_picked_up() {
    let dir = tempfile::tempdir().unwrap();
    let sub = || FakeSub {
        expires_in: 10,
        ..FakeSub::default()
    };
    let (here, here_sub) = manager(dir.path(), sub());
    link_by_paste(&here).await;
    let (there, there_sub) = manager(dir.path(), sub());
    let refreshed = there
        .inference_credential(ProviderKind::Claude)
        .await
        .unwrap();
    assert_eq!(refreshed.bearer, "access-1");
    assert_eq!(there_sub.refreshes.load(Ordering::SeqCst), 1);

    let credential = here
        .inference_credential(ProviderKind::Claude)
        .await
        .unwrap();
    assert_eq!(credential.bearer, "access-1");
    assert_eq!(
        here_sub.refreshes.load(Ordering::SeqCst),
        0,
        "the spent refresh token kept here is not used"
    );
}

#[tokio::test]
async fn revoked_refresh_marks_link_for_reauth() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, sub) = manager(
        dir.path(),
        FakeSub {
            expires_in: 10,
            ..FakeSub::default()
        },
    );
    link_by_paste(&manager).await;
    *sub.refresh_outcome.lock().unwrap() = Some(ProviderError::ReauthRequired("revoked".into()));
    let err = manager
        .inference_credential(ProviderKind::Claude)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::ReauthRequired(_)));
    let link = manager.link(ProviderKind::Claude).unwrap();
    assert!(!link.usable());
    assert!(manager.vault().tokens(&link).is_none());
    assert!(!manager.summary().can_use(ProviderKind::Claude));
}

#[tokio::test]
async fn transient_refresh_failure_keeps_unexpired_token() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, sub) = manager(
        dir.path(),
        FakeSub {
            expires_in: 30,
            ..FakeSub::default()
        },
    );
    link_by_paste(&manager).await;
    *sub.refresh_outcome.lock().unwrap() = Some(ProviderError::Unavailable("down".into()));
    let credential = manager
        .inference_credential(ProviderKind::Claude)
        .await
        .unwrap();
    assert_eq!(credential.bearer, "access-0");
    assert!(manager.link(ProviderKind::Claude).unwrap().usable());
}

#[tokio::test]
async fn plan_tier_is_recorded_and_refreshed() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, sub) = manager(
        dir.path(),
        FakeSub {
            expires_in: 3600,
            tier: Mutex::new(Some("Pro".into())),
            ..FakeSub::default()
        },
    );
    let link = link_by_paste(&manager).await;
    assert_eq!(link.plan.as_deref(), Some("Pro"));
    assert_eq!(manager.summary().plan_names, ["Claude Pro"]);

    *sub.tier.lock().unwrap() = Some("Max 20x".into());
    manager.refresh_plan_tiers().await;
    assert_eq!(manager.summary().plan_names, ["Claude Max 20x"]);

    *sub.tier.lock().unwrap() = None;
    manager.refresh_plan_tiers().await;
    assert_eq!(
        manager.summary().plan_names,
        ["Claude Max 20x"],
        "an unreadable tier keeps the stored one"
    );
}

#[tokio::test]
async fn failed_exchange_and_unlink() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, _) = manager(
        dir.path(),
        FakeSub {
            expires_in: 3600,
            ..FakeSub::default()
        },
    );
    let pending = manager
        .begin_link(ProviderKind::Claude, LinkMode::Paste)
        .await
        .unwrap();
    let state = url_state(pending.url());
    assert!(manager
        .finish_link(pending, Some(&format!("bad#{state}")))
        .await
        .is_err());
    assert!(manager.links().is_empty());

    link_by_paste(&manager).await;
    assert!(manager
        .unlink(ProviderKind::Claude)
        .await
        .unwrap()
        .is_some());
    assert!(manager.links().is_empty());
    assert!(manager
        .unlink(ProviderKind::Claude)
        .await
        .unwrap()
        .is_none());
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

#[tokio::test]
async fn openrouter_pasted_key_links_and_persists() {
    let (base, exchanges) = openrouter_fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let manager = openrouter_manager(dir.path(), &base);
    let pending = manager
        .begin_link(ProviderKind::OpenRouter, LinkMode::Paste)
        .await
        .unwrap();
    assert!(pending.needs_paste());
    let link = manager
        .finish_link(pending, Some(&format!("  {OPENROUTER_KEY}\n")))
        .await
        .unwrap();
    assert!(
        exchanges.lock().unwrap().is_empty(),
        "a key is not exchanged"
    );
    assert_eq!(link.provider, ProviderKind::OpenRouter);
    assert_eq!(link.owner, Owner::Local);
    assert_eq!(link.label, "Aura key");
    assert_eq!(link.plan.as_deref(), Some("credits"));

    let reopened = openrouter_manager(dir.path(), &base);
    assert_eq!(
        reopened.link(ProviderKind::OpenRouter).map(|l| l.id),
        Some(link.id)
    );
    assert!(reopened
        .summary()
        .subscriptions
        .contains(&ProviderKind::OpenRouter));
    assert_eq!(
        reopened.summary().plan_names,
        ["OpenRouter"],
        "the badge omits the OpenRouter tier"
    );
    let credential = reopened
        .inference_credential(ProviderKind::OpenRouter)
        .await
        .unwrap();
    assert_eq!(credential.bearer, OPENROUTER_KEY);
    assert!(credential
        .headers
        .contains(&("X-Title".into(), aura_auth_openrouter::APP_TITLE.into())));
}

#[tokio::test]
async fn openrouter_key_is_used_without_refresh() {
    let (base, _) = openrouter_fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(CountingOpenRouter {
        inner: OpenRouterProvider::fixture(&base).unwrap(),
        refreshes: AtomicUsize::new(0),
    });
    let manager = AuthManager::with_identity(Vault::files_at(dir.path()), Arc::new(FakeZos))
        .with_subscription(provider.clone());
    let link = manager
        .link_api_key(ProviderKind::OpenRouter, OPENROUTER_KEY)
        .await
        .unwrap();
    let tokens = manager.vault().tokens(&link).unwrap();
    assert_eq!(tokens.refresh_token, None);
    assert!(!tokens.expires_within(Utc::now(), REFRESH_MARGIN_SECS));

    for _ in 0..2 {
        let credential = manager
            .inference_credential(ProviderKind::OpenRouter)
            .await
            .unwrap();
        assert_eq!(credential.bearer, OPENROUTER_KEY);
    }
    assert_eq!(provider.refreshes.load(Ordering::SeqCst), 0);
    assert!(manager.link(ProviderKind::OpenRouter).unwrap().usable());
}

#[tokio::test]
async fn openrouter_unlink_and_rejected_key() {
    let (base, _) = openrouter_fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let manager = openrouter_manager(dir.path(), &base);

    let err = manager
        .link_api_key(ProviderKind::OpenRouter, "sk-or-test-unknown-key")
        .await
        .unwrap_err();
    assert!(!err.to_string().contains("sk-or-test-unknown-key"), "{err}");
    assert!(manager.links().is_empty());

    manager
        .link_api_key(ProviderKind::OpenRouter, OPENROUTER_KEY)
        .await
        .unwrap();
    let removed = manager.unlink(ProviderKind::OpenRouter).await.unwrap();
    assert_eq!(removed.map(|l| l.provider), Some(ProviderKind::OpenRouter));
    assert!(manager.link(ProviderKind::OpenRouter).is_none());
    assert!(matches!(
        manager.inference_credential(ProviderKind::OpenRouter).await,
        Err(ProviderError::NotLinked(ProviderKind::OpenRouter))
    ));
}

#[tokio::test]
async fn openrouter_pasted_code_is_exchanged() {
    let (base, exchanges) = openrouter_fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let manager = openrouter_manager(dir.path(), &base);
    let pending = manager
        .begin_link(ProviderKind::OpenRouter, LinkMode::Paste)
        .await
        .unwrap();
    let link = manager
        .finish_link(pending, Some("the-code"))
        .await
        .unwrap();
    let sent = exchanges.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["code"], "the-code");
    assert_eq!(link.subject, "user-or");
    let credential = manager
        .inference_credential(ProviderKind::OpenRouter)
        .await
        .unwrap();
    assert_eq!(credential.bearer, OPENROUTER_KEY);
}

#[tokio::test]
async fn a_wider_margin_refreshes_early_for_the_named_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, sub) = manager(
        dir.path(),
        FakeSub {
            expires_in: 600,
            ..FakeSub::default()
        },
    );
    link_by_paste(&manager).await;
    let local = aura_core_auth::Owner::Local;
    assert_eq!(manager.linked_plans(&local), vec![ProviderKind::Claude]);
    let bob = aura_core_auth::Owner::Zos {
        user_id: "bob".into(),
    };
    assert!(manager.linked_plans(&bob).is_empty());

    // Ten minutes left: fine for a request, too short for the cloud hub.
    let now = manager
        .inference_credential(ProviderKind::Claude)
        .await
        .unwrap();
    assert_eq!(now.bearer, "access-0");
    assert_eq!(sub.refreshes.load(Ordering::SeqCst), 0);
    let early = manager
        .inference_credential_for(&local, ProviderKind::Claude, 900)
        .await
        .unwrap();
    assert_eq!(early.bearer, "access-1");
    assert_eq!(sub.refreshes.load(Ordering::SeqCst), 1);

    let err = manager
        .inference_credential_for(&bob, ProviderKind::Claude, 900)
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProviderError::NotLinked(ProviderKind::Claude)),
        "{err:?}"
    );
}
