use super::*;

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
