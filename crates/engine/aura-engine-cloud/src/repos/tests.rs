use super::*;
use crate::fake::FakeProvider;
use crate::github::FakeRepoTokens;

fn repo(name: &str) -> RepoRef {
    RepoRef {
        owner: "acme".into(),
        name: name.into(),
    }
}

fn sync() -> (RepoSync, FakeProvider, Arc<FakeRepoTokens>) {
    let fake = FakeProvider::new();
    let tokens = Arc::new(FakeRepoTokens::new("aura-agents"));
    let sync = RepoSync::new(Arc::new(fake.clone()), Some(tokens.clone()), "inst01abcdef");
    (sync, fake, tokens)
}

fn in_minutes(minutes: u64) -> u64 {
    now_ms() + minutes * 60 * 1000
}

#[tokio::test]
async fn a_repo_gets_one_secret_bound_to_github_headers() {
    let (sync, fake, tokens) = sync();
    tokens.install(&repo("app"), "ghs_one", in_minutes(60));
    let status = sync.ensure(&repo("app")).await.expect("ensure");
    assert!(status.linked);
    assert_eq!(status.error, None);
    assert!(names::is_repo_secret(&status.secret));
    assert!(
        status.secret.starts_with("AURA_GH_INST01_"),
        "{}",
        status.secret
    );
    assert_eq!(
        fake.secret_value(&status.secret).as_deref(),
        Some("ghs_one")
    );
    let secret = &fake.secrets()[&status.secret];
    assert_eq!(secret.domains, ["github.com", "api.github.com"]);
    assert_eq!(secret.replace_in, ["headers"]);
    assert!(!format!("{sync:?}{status:?}").contains("ghs_"));
}

#[tokio::test]
async fn tokens_are_reminted_only_inside_the_margin() {
    let (sync, fake, tokens) = sync();
    tokens.install(&repo("app"), "ghs_one", in_minutes(60));
    let name = sync.ensure(&repo("app")).await.expect("ensure").secret;
    sync.tick(&[repo("app")]).await;
    assert_eq!(tokens.mints(&repo("app")), 1, "an hour left: no mint");

    // Twenty minutes left: inside the 30-minute margin.
    sync.repos
        .lock()
        .get_mut(&repo("app"))
        .expect("state")
        .expires_at_ms = Some(in_minutes(20));
    tokens.install(&repo("app"), "ghs_two", in_minutes(60));
    sync.tick(&[repo("app")]).await;
    assert_eq!(tokens.mints(&repo("app")), 2);
    assert_eq!(fake.secret_value(&name).as_deref(), Some("ghs_two"));
    assert_eq!(
        fake.calls_matching("SetSecret(").len(),
        2,
        "rotation keeps the name"
    );
}

#[tokio::test]
async fn a_repo_without_the_app_holds_unlinked_with_the_install_link() {
    let (sync, fake, tokens) = sync();
    let status = sync.ensure(&repo("private")).await.expect("ensure");
    assert!(!status.linked);
    assert_eq!(fake.secret_value(&status.secret).as_deref(), Some(UNLINKED));
    assert_eq!(
        status.install_url.as_deref(),
        Some("https://github.com/apps/aura-agents/installations/new")
    );
    assert!(status.error.is_some_and(|e| e.contains("isn't installed")));

    // Backing off: the next tick doesn't ask GitHub again.
    sync.tick(&[repo("private")]).await;
    assert_eq!(tokens.mints(&repo("private")), 1);

    // Installed later: a placement (strict) picks it up at once.
    tokens.install(&repo("private"), "ghs_now", in_minutes(60));
    let status = sync.ensure(&repo("private")).await.expect("ensure");
    assert!(status.linked);
    assert_eq!(status.install_url, None);
    assert_eq!(
        fake.secret_value(&status.secret).as_deref(),
        Some("ghs_now")
    );
}

#[tokio::test]
async fn a_failing_mint_keeps_the_old_token_and_backs_off_alone() {
    let (sync, fake, tokens) = sync();
    tokens.install(&repo("a"), "ghs_a1", in_minutes(10));
    tokens.install(&repo("b"), "ghs_b1", in_minutes(10));
    let a = sync.ensure(&repo("a")).await.expect("a").secret;
    let b = sync.ensure(&repo("b")).await.expect("b").secret;
    tokens.fail(
        &repo("a"),
        GithubError::Transport {
            op: "token mint",
            message: "timed out".into(),
        },
    );
    tokens.install(&repo("b"), "ghs_b2", in_minutes(60));
    sync.tick(&[repo("a"), repo("b")]).await;
    assert_eq!(fake.secret_value(&a).as_deref(), Some("ghs_a1"), "kept");
    assert_eq!(fake.secret_value(&b).as_deref(), Some("ghs_b2"));
    assert!(sync
        .status(&repo("a"))
        .and_then(|s| s.error)
        .is_some_and(|e| e.contains("timed out")));
    sync.tick(&[repo("a")]).await;
    assert_eq!(tokens.mints(&repo("a")), 2, "backing off");
}

#[tokio::test]
async fn without_an_app_every_repo_holds_unlinked() {
    let fake = FakeProvider::new();
    let sync = RepoSync::new(Arc::new(fake.clone()), None, "inst01");
    assert!(!sync.configured());
    let status = sync.ensure(&repo("app")).await.expect("ensure");
    assert!(!status.linked);
    assert!(status
        .error
        .is_some_and(|e| e.contains("AURA_GITHUB_APP_ID")));
    assert_eq!(fake.secret_value(&status.secret).as_deref(), Some(UNLINKED));
    assert_eq!(sync.identity().await, None);
}
