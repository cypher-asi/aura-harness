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
