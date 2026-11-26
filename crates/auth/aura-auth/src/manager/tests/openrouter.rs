use super::*;

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
