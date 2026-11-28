use super::*;
use aura_core_auth::LinkStatus;
use chrono::Utc;

fn link(id: &str, owner: Owner, provider: ProviderKind) -> LinkedAccount {
    LinkedAccount {
        id: id.into(),
        provider,
        owner,
        subject: "sub".into(),
        label: "a@b.c".into(),
        client_id: "client".into(),
        plan_enabled: true,
        expires_at: Utc::now(),
        created_at: Utc::now(),
        status: LinkStatus::Active,
        plan: None,
    }
}

fn tokens(access: &str) -> OAuthTokens {
    OAuthTokens {
        access_token: access.into(),
        refresh_token: Some("r".into()),
        id_token: None,
        scope: String::new(),
        expires_at: Utc::now(),
        issued_at: None,
    }
}

fn zos(user: &str) -> Owner {
    Owner::Zos {
        user_id: user.into(),
    }
}

#[test]
fn legacy_zos_file_is_read_in_place() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
            dir.path().join("credentials.json"),
            r#"{"access_token":"jwt","user_id":"u","display_name":"n","primary_zid":"z","created_at":"2025-01-01T00:00:00Z"}"#,
        )
        .unwrap();
    let vault = Vault::files_at(dir.path());
    assert_eq!(vault.zos_session().unwrap().access_token, "jwt");
    vault.clear_zos_session().unwrap();
    assert!(vault.zos_session().is_none());
}

#[test]
fn the_zos_session_is_kept_for_a_while_then_read_again() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("credentials.json");
    std::fs::write(
            &file,
            r#"{"access_token":"jwt","user_id":"u","display_name":"n","primary_zid":"z","created_at":"2025-01-01T00:00:00Z"}"#,
        )
        .unwrap();
    let vault = Vault::files_at(dir.path());
    assert!(vault.zos_session().is_some());
    std::fs::remove_file(&file).unwrap();
    assert!(vault.zos_session().is_some(), "kept, not read again");
    vault.kept().zos.as_mut().unwrap().0 -= ZOS_SESSION_TTL;
    assert!(vault.zos_session().is_none(), "read again once stale");
}

#[test]
fn tokens_are_read_again_when_the_index_has_another_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::files_at(dir.path());
    let l = link("l1", Owner::Local, ProviderKind::Claude);
    vault.put_link(&l, &tokens("a1")).unwrap();
    let other_process = Vault::files_at(dir.path());
    let rotated = tokens("a2");
    other_process.save_tokens(&l, &rotated).unwrap();
    assert_eq!(vault.tokens(&l).unwrap().access_token, "a1");
    let refreshed = LinkedAccount {
        expires_at: rotated.expires_at,
        ..l
    };
    assert_eq!(vault.tokens(&refreshed).unwrap().access_token, "a2");
}

#[test]
fn links_are_scoped_by_owner_and_replaced_per_provider() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::files_at(dir.path());
    let first = link("l1", zos("u"), ProviderKind::Chatgpt);
    vault.put_link(&first, &tokens("a1")).unwrap();
    vault
        .put_link(
            &link("l2", Owner::Local, ProviderKind::Chatgpt),
            &tokens("a2"),
        )
        .unwrap();
    assert_eq!(vault.links_for(&zos("u")).len(), 1);
    assert_eq!(vault.links_for(&Owner::Local).len(), 1);
    assert!(vault.links_for(&zos("other")).is_empty());

    let second = link("l3", zos("u"), ProviderKind::Chatgpt);
    vault.put_link(&second, &tokens("a3")).unwrap();
    assert_eq!(
        vault.link(&zos("u"), ProviderKind::Chatgpt).unwrap().id,
        "l3"
    );
    assert!(
        vault.tokens(&first).is_none(),
        "replaced link secret removed"
    );
    assert_eq!(vault.tokens(&second).unwrap().access_token, "a3");
}

#[cfg(unix)]
#[test]
fn secrets_are_private_files() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::files_at(dir.path());
    let l = link("l1", Owner::Local, ProviderKind::Claude);
    vault.put_link(&l, &tokens("secret")).unwrap();
    let secret = dir.path().join("auth/secrets/l1.json");
    let mode = std::fs::metadata(&secret).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let index = std::fs::read_to_string(dir.path().join("auth/index.json")).unwrap();
    assert!(!index.contains("secret"), "index must not hold tokens");
    let bytes = std::fs::read(&secret).unwrap();
    assert!(
        !String::from_utf8_lossy(&bytes).contains("secret"),
        "the token file is sealed"
    );
    assert_eq!(vault.tokens(&l).unwrap().access_token, "secret");
}

#[test]
fn a_plain_token_file_from_before_still_signs_in_and_is_sealed() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::files_at(dir.path());
    let l = link("l1", Owner::Local, ProviderKind::Claude);
    vault.put_link(&l, &tokens("first")).unwrap();
    // A token file written before sealing, plain.
    let secret = dir.path().join("auth/secrets/l1.json");
    let plain = serde_json::to_string(&tokens("plain-old")).unwrap();
    std::fs::write(&secret, &plain).unwrap();
    let fresh = Vault::files_at(dir.path());
    assert_eq!(fresh.tokens(&l).unwrap().access_token, "plain-old");
    let bytes = std::fs::read(&secret).unwrap();
    assert!(
        !String::from_utf8_lossy(&bytes).contains("plain-old"),
        "migrated"
    );
    assert_eq!(
        Vault::files_at(dir.path()).tokens(&l).unwrap().access_token,
        "plain-old"
    );
}

#[test]
fn move_links_keeps_existing_target_links() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::files_at(dir.path());
    vault
        .put_link(
            &link("local-gpt", Owner::Local, ProviderKind::Chatgpt),
            &tokens("a"),
        )
        .unwrap();
    vault
        .put_link(
            &link("local-claude", Owner::Local, ProviderKind::Claude),
            &tokens("b"),
        )
        .unwrap();
    vault
        .put_link(
            &link("zos-claude", zos("u"), ProviderKind::Claude),
            &tokens("c"),
        )
        .unwrap();
    assert_eq!(vault.move_links(&Owner::Local, &zos("u")).unwrap(), 1);
    assert!(vault.links_for(&Owner::Local).is_empty());
    let mine = vault.links_for(&zos("u"));
    assert_eq!(mine.len(), 2);
    assert_eq!(
        vault.link(&zos("u"), ProviderKind::Claude).unwrap().id,
        "zos-claude"
    );
}

#[test]
fn issued_clients_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::files_at(dir.path());
    let owner = Owner::Local;
    assert!(vault.issued_client(ProviderKind::Chatgpt, &owner).is_none());
    vault
        .set_issued_client(ProviderKind::Chatgpt, &owner, Some("issued"))
        .unwrap();
    assert_eq!(
        vault
            .issued_client(ProviderKind::Chatgpt, &owner)
            .as_deref(),
        Some("issued")
    );
    vault
        .set_issued_client(ProviderKind::Chatgpt, &owner, None)
        .unwrap();
    assert!(vault.issued_client(ProviderKind::Chatgpt, &owner).is_none());
}
