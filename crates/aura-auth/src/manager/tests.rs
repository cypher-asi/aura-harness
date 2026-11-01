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
