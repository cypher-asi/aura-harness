use super::*;

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
