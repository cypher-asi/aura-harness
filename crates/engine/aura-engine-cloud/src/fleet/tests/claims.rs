use super::*;

#[tokio::test]
async fn a_machine_lists_one_repo_secret_and_its_claim_names_the_branch() {
    let (fleet, fake, _) = fleet(Duration::from_secs(300));
    let claims = Arc::new(Mutex::new(Vec::<NodeClaim>::new()));
    let seen = Arc::clone(&claims);
    fake.on_exec(Arc::new(move |_, _, stdin| {
        if let Some(claim) = stdin.and_then(|s| serde_json::from_slice(s).ok()) {
            seen.lock().push(claim);
        }
        crate::provider::ExecOutput::default()
    }));
    let plan = crate::names::plan_secret(ALICE, "inst-1", aura_core_auth::ProviderKind::Claude)
        .expect("name");
    let repo = crate::names::repo_secret("inst-1", "acme/app");
    for name in [&plan, &repo] {
        fake.set_secret(&crate::provider::HostSecret {
            name: name.clone(),
            value: zeroize::Zeroizing::new("unlinked".into()),
            domains: vec!["github.com".into()],
            replace_in: vec!["headers".into()],
        })
        .await
        .expect("secret");
    }
    let mut aa = request("aa", MachineOrigin::Fresh);
    aa.secrets = vec![plan.clone()];
    aa.repo_secret = Some(repo.clone());
    aa.git = Some(GitClaim {
        branch: "aura/reviewer-re36".into(),
        remote_url: Some("https://github.com/acme/app.git".into()),
        env: Some(repo.clone()),
        placeholder: None,
        author_name: Some("aura-agents[bot]".into()),
        author_email: None,
    });
    fleet.provision(aa).await.expect("provision");
    assert_eq!(
        fake.machine_secrets("aura-re36-aa"),
        vec![plan, repo.clone()]
    );
    let git = claims.lock()[0].git.clone().expect("git claim");
    assert_eq!(git.branch, "aura/reviewer-re36");
    assert_eq!(git.env.as_deref(), Some(repo.as_str()));

    // A plan secret passed as the repository secret is refused.
    let mut bb = request("bb", MachineOrigin::Fresh);
    bb.repo_secret = Some(
        crate::names::plan_secret(BOB, "inst-1", aura_core_auth::ProviderKind::Claude)
            .expect("name"),
    );
    let err = fleet.provision(bb).await.expect_err("not a repo secret");
    assert!(err.to_string().contains("repository secret"), "{err}");
}

#[tokio::test]
async fn machines_carry_their_owner_and_only_their_owners_secrets() {
    let (fleet, fake, ledger) = fleet(Duration::from_secs(300));
    let secret = |owner: &str| {
        crate::names::plan_secret(owner, "inst-1", aura_core_auth::ProviderKind::Claude)
            .expect("name")
    };
    for owner in [ALICE, BOB] {
        fake.set_secret(&crate::provider::HostSecret {
            name: secret(owner),
            value: zeroize::Zeroizing::new("unlinked".into()),
            domains: vec!["api.anthropic.com".into()],
            replace_in: Vec::new(),
        })
        .await
        .expect("secret");
    }
    let mut aa = request("aa", MachineOrigin::Fresh);
    aa.secrets = vec![secret(ALICE)];
    let view = fleet.provision(aa).await.expect("aa");
    assert_eq!(view.owner, ALICE);
    assert_eq!(
        view.machine.labels.get(LABEL_OWNER).map(String::as_str),
        Some(ALICE)
    );
    assert_eq!(fake.machine_secrets("aura-re36-aa"), vec![secret(ALICE)]);
    assert!(ledger
        .rows()
        .iter()
        .all(|(_, row)| row.owner.as_deref() == Some(ALICE)));

    // Bob's secret is refused for Alice's machine.
    let mut cc = request("cc", MachineOrigin::Fresh);
    cc.secrets = vec![secret(BOB)];
    let err = fleet.provision(cc).await.expect_err("foreign secret");
    assert!(err.to_string().contains("not its owner's plan"), "{err}");

    // Bob can't fork Alice's machine.
    let mut bb = request("bb", MachineOrigin::Fork("aura-re36-aa".into()));
    bb.owner = BOB.into();
    let err = fleet.provision(bb).await.expect_err("cross-owner fork");
    assert!(err.to_string().contains("another user's machine"), "{err}");
    // Alice can, and the fork keeps her secrets.
    let dd = request("dd", MachineOrigin::Fork("aura-re36-aa".into()));
    fleet.provision(dd).await.expect("same-owner fork");
    assert_eq!(fake.machine_secrets("aura-re36-dd"), vec![secret(ALICE)]);

    assert_eq!(fleet.views_of(ALICE).len(), 2);
    assert!(fleet.views_of(BOB).is_empty());
    assert!(fleet.project_mesh_token(ALICE, "p1").is_some());
    assert!(
        fleet.project_mesh_token(BOB, "p1").is_none(),
        "mesh tokens are per owner"
    );
}
