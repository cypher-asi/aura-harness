use super::*;

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

/// A manager linked to tokens issued 5 hours ago with 3 hours left:
/// past half their lifetime, outside any margin.
async fn half_used() -> (tempfile::TempDir, AuthManager, Arc<FakeSub>) {
    let dir = tempfile::tempdir().unwrap();
    let (manager, sub) = manager(
        dir.path(),
        FakeSub {
            expires_in: 3 * 3600,
            issued_ago: 5 * 3600,
            ..FakeSub::default()
        },
    );
    link_by_paste(&manager).await;
    (dir, manager, sub)
}

#[tokio::test]
async fn tokens_for_machines_are_refreshed_once_half_their_lifetime_is_over() {
    let (_dir, manager, sub) = half_used().await;
    let local = aura_core_auth::Owner::Local;
    let kept = manager
        .inference_credential_for(&local, ProviderKind::Claude, 900)
        .await
        .unwrap();
    assert_eq!(kept.bearer, "access-0");
    assert_eq!(sub.refreshes.load(Ordering::SeqCst), 0);

    for _ in 0..2 {
        let ahead = manager
            .inference_credential_ahead(&local, ProviderKind::Claude, 900)
            .await
            .unwrap();
        assert_eq!(ahead.bearer, "access-1");
    }
    assert_eq!(
        sub.refreshes.load(Ordering::SeqCst),
        1,
        "the new token has most of its lifetime"
    );
}

#[tokio::test]
async fn an_early_refresh_that_fails_keeps_the_token_and_waits() {
    let (_dir, manager, sub) = half_used().await;
    let local = aura_core_auth::Owner::Local;
    *sub.refresh_outcome.lock().unwrap() = Some(ProviderError::ReauthRequired("revoked".into()));
    for _ in 0..2 {
        let kept = manager
            .inference_credential_ahead(&local, ProviderKind::Claude, 900)
            .await
            .unwrap();
        assert_eq!(kept.bearer, "access-0");
    }
    assert_eq!(
        sub.refreshes.load(Ordering::SeqCst),
        1,
        "the next early refresh waits"
    );
    assert!(manager.link(ProviderKind::Claude).unwrap().usable());
}
