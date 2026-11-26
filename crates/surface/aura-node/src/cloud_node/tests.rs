use super::*;
use axum::http::HeaderValue;

fn node() -> Arc<CloudNode> {
    CloudNode::new(&NodeConfig {
        cloud_node: true,
        ..NodeConfig::default()
    })
}

fn bearer(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).expect("header"),
    );
    headers
}

fn claim(seal: Vec<String>) -> NodeClaim {
    NodeClaim {
        node_id: "aura-re36-3f9c".into(),
        node_token: "node-token-0123456789".into(),
        mesh_token: "mesh-token-0123456789".into(),
        seal,
        owner: None,
        plans: Vec::new(),
        git: None,
    }
}

#[test]
fn an_unclaimed_node_refuses_everything() {
    let node = node();
    assert_eq!(
        node.authorize(&bearer("anything"), "/v1/host"),
        Err(StatusCode::SERVICE_UNAVAILABLE)
    );
}

#[test]
fn a_claimed_node_takes_its_tokens_only() {
    let node = node();
    let agent = AgentId::new([9; 32]);
    let answer = node.apply_claim(claim(vec![agent.to_hex(), "zz".into()]), 8080);
    assert_eq!(answer.sealed, 1);
    assert_eq!(node.node_id().as_deref(), Some("aura-re36-3f9c"));
    assert!(node.is_sealed(&agent));
    assert_eq!(
        node.authorize(&bearer("node-token-0123456789"), "/v1/host"),
        Ok(Caller::Hub)
    );
    assert_eq!(
        node.authorize(&bearer("mesh-token-0123456789"), "/v1/mesh"),
        Ok(Caller::Peer)
    );
    assert_eq!(
        node.authorize(&bearer("mesh-token-0123456789"), "/v1/host"),
        Err(StatusCode::UNAUTHORIZED),
        "peers only reach the mesh"
    );
    assert_eq!(
        node.authorize(&HeaderMap::new(), "/v1/host"),
        Err(StatusCode::UNAUTHORIZED)
    );
}

#[test]
fn a_new_claim_replaces_the_old_tokens() {
    let node = node();
    node.apply_claim(claim(Vec::new()), 8080);
    let mut second = claim(Vec::new());
    second.node_token = "rotated-token-0123456789".into();
    node.apply_claim(second, 8080);
    assert_eq!(
        node.authorize(&bearer("node-token-0123456789"), "/v1/host"),
        Err(StatusCode::UNAUTHORIZED)
    );
    assert_eq!(
        node.authorize(&bearer("rotated-token-0123456789"), "/v1/host"),
        Ok(Caller::Hub)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn claims_travel_over_the_socket() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = claim_socket_path(dir.path());
    let node = node();
    let server = tokio::spawn(serve_claims(Arc::clone(&node), path.clone(), 8080));
    for _ in 0..50 {
        if path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let bytes = serde_json::to_vec(&claim(Vec::new())).expect("json");
    let answer = send_claim(&path, &bytes).await.expect("claimed");
    let claimed: NodeClaimed = serde_json::from_slice(&answer).expect("answer");
    assert_eq!(claimed.node_id, "aura-re36-3f9c");
    assert_eq!(claimed.port, 8080);
    assert_eq!(claimed.protocol, aura_core_protocol::cloud::NODE_PROTOCOL);
    assert_eq!(node.node_id().as_deref(), Some("aura-re36-3f9c"));
    let err = send_claim(&path, b"not json").await.expect_err("bad claim");
    assert!(err.to_string().contains("bad claim"), "{err}");
    server.abort();
}

fn claim_with(owner: &str, env: &str) -> NodeClaim {
    NodeClaim {
        node_id: "aura-x".into(),
        node_token: "t".into(),
        mesh_token: "m".into(),
        seal: Vec::new(),
        owner: Some(owner.into()),
        plans: vec![aura_core_protocol::cloud::PlanTemplate {
            provider: "claude".into(),
            env: env.into(),
            headers: Vec::new(),
            system_preamble: None,
            placeholder: None,
        }],
        git: None,
    }
}

const REPO_SECRET: &str = "AURA_GH_INST01_0A1B2C3D";

fn git_claim(placeholder: Option<&str>) -> GitClaim {
    GitClaim {
        branch: "aura/reviewer-re36".into(),
        remote_url: Some("https://github.com/acme/app.git".into()),
        env: Some(REPO_SECRET.into()),
        placeholder: placeholder.map(str::to_string),
        author_name: Some("aura-agents[bot]".into()),
        author_email: Some("4242+aura-agents[bot]@users.noreply.github.com".into()),
    }
}

#[test]
fn the_git_placeholder_is_filled_like_the_plans() {
    let mut claim = claim(Vec::new());
    claim.git = Some(git_claim(Some("bxds_sent-by-the-hub")));
    fill_placeholders(&mut claim, |name| {
        (name == REPO_SECRET).then(|| "bxds_gh123".to_string())
    })
    .expect("filled");
    assert_eq!(
        claim.git.as_ref().and_then(|g| g.placeholder.as_deref()),
        Some("bxds_gh123")
    );

    let mut unset = claim.clone();
    fill_placeholders(&mut unset, |_| None).expect("unset");
    assert_eq!(unset.git.and_then(|g| g.placeholder), None);

    let mut leaked = claim.clone();
    let err =
        fill_placeholders(&mut leaked, |_| Some("ghs_realtoken".into())).expect_err("real token");
    assert!(
        err.contains("not a boxd placeholder") && !err.contains("ghs_"),
        "{err}"
    );

    let mut foreign = claim.clone();
    if let Some(git) = foreign.git.as_mut() {
        git.env = Some("AURA_A11CE000_INST01_CLAUDE".into());
    }
    let err = fill_placeholders(&mut foreign, |_| Some("bxds_x".into())).expect_err("plan");
    assert!(err.contains("not a repository secret"), "{err}");
}

#[test]
fn git_files_hold_only_the_placeholder_and_only_the_user_reads_them() {
    let home = tempfile::tempdir().expect("tempdir");
    write_git_files(home.path(), &git_claim(Some("bxds_gh123"))).expect("written");
    let files = GitFiles::under(home.path());
    let config = std::fs::read_to_string(&files.config).expect("config");
    assert!(config.contains("name = aura-agents[bot]"), "{config}");
    assert!(config.contains("email = 4242+aura-agents[bot]@users.noreply.github.com"));
    assert!(config.contains("default = current"));
    assert!(config.contains(&format!(
        "helper = store --file {}",
        files.credentials.display()
    )));
    assert!(!config.contains("bxds_"), "no token in git's own settings");
    assert_eq!(
        std::fs::read_to_string(&files.credentials).expect("store"),
        "https://x-access-token:bxds_gh123@github.com\n"
    );
    let hosts = std::fs::read_to_string(&files.gh_hosts).expect("hosts");
    assert!(hosts.contains("oauth_token: \"bxds_gh123\""), "{hosts}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for file in [&files.credentials, &files.gh_hosts] {
            let mode = std::fs::metadata(file).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{}", file.display());
        }
    }

    // A re-claim without a token takes the token files away.
    write_git_files(home.path(), &git_claim(None)).expect("rewritten");
    assert!(!files.credentials.exists() && !files.gh_hosts.exists());
    let config = std::fs::read_to_string(&files.config).expect("config");
    assert!(!config.contains("credential"), "{config}");

    let err = write_git_files(home.path(), &git_claim(Some("ghs_real"))).expect_err("real");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(!files.credentials.exists());
}

#[test]
fn the_claimed_branch_is_kept_without_the_placeholder() {
    let node = node();
    assert_eq!(node.git_branch(), None);
    let mut claimed = claim(Vec::new());
    claimed.git = Some(git_claim(Some("bxds_gh123")));
    node.apply_claim(claimed, 8080);
    assert_eq!(node.git_branch().as_deref(), Some("aura/reviewer-re36"));
    let kept = node.git.read().expect("lock").clone().expect("git");
    assert_eq!(kept.placeholder, None);
}

#[test]
fn placeholders_are_filled_and_real_tokens_refused() {
    let env = "AURA_A11CE000_INST01_CLAUDE";
    let mut claim = claim_with("a11ce000", env);
    let filled = fill_placeholders(&mut claim, |name| {
        (name == env).then(|| "bxds_abc123".to_string())
    })
    .expect("filled");
    assert_eq!(filled, 1);
    assert_eq!(claim.plans[0].placeholder.as_deref(), Some("bxds_abc123"));

    let mut unset = claim_with("a11ce000", env);
    assert_eq!(fill_placeholders(&mut unset, |_| None).expect("unset"), 0);
    assert!(unset.plans[0].placeholder.is_none());

    let mut leaked = claim_with("a11ce000", env);
    let err = fill_placeholders(&mut leaked, |_| Some("sk-ant-oat01-real".into()))
        .expect_err("real token");
    assert!(err.contains("not a boxd placeholder"), "{err}");
    assert!(!err.contains("sk-ant"), "the value is never echoed");

    let mut foreign = claim_with("a11ce000", "AURA_B0B00000_INST01_CLAUDE");
    let err = fill_placeholders(&mut foreign, |_| Some("bxds_x".into())).expect_err("foreign");
    assert!(err.contains("not this machine owner's"), "{err}");
}

#[tokio::test]
async fn the_plan_resolver_answers_from_the_claim_only() {
    use aura_core_auth::{AuthError, CredentialResolver, ProviderKind};
    let node = CloudNode::new(&NodeConfig {
        cloud_node: true,
        ..NodeConfig::default()
    });
    let plans = Arc::new(PlanResolver::default());
    node.attach_plans(Arc::clone(&plans));
    let err = plans
        .inference_credential(ProviderKind::Claude)
        .await
        .expect_err("before the claim");
    assert!(matches!(err, AuthError::NotLinked(ProviderKind::Claude)));

    let mut claim = claim_with("a11ce000", "AURA_A11CE000_INST01_CLAUDE");
    claim.plans[0].headers = vec![("anthropic-beta".into(), "oauth-2025-04-20".into())];
    claim.plans[0].system_preamble = Some("You are Claude Code".into());
    claim.plans[0].placeholder = Some("bxds_abc".into());
    node.apply_claim(claim, 8080);
    let credential = plans
        .inference_credential(ProviderKind::Claude)
        .await
        .expect("claude");
    assert_eq!(credential.bearer, "bxds_abc");
    assert_eq!(credential.headers[0].0, "anthropic-beta");
    assert_eq!(
        credential.system_preamble.as_deref(),
        Some("You are Claude Code")
    );
    assert_eq!(plans.owner().as_deref(), Some("a11ce000"));
    assert!(plans
        .inference_credential(ProviderKind::Chatgpt)
        .await
        .is_err());
}
