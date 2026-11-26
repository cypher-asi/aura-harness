use super::*;

#[tokio::test]
async fn a_project_on_github_gives_its_machine_a_repository_secret_and_the_bot() {
    let (hub, fake, dir, repo) = github_hub();
    let project = dir.path().join("project");
    project_repo(&project, Some("git@github.com:acme/proj.git"));
    let share = hub
        .share_repo(&request_in(&project), &BootReporter::none())
        .await
        .expect("share");
    assert_eq!(share.repo, Some(repo.clone()));
    let secret = share.secret.clone().expect("secret");
    assert!(names::is_repo_secret(&secret), "{secret}");
    assert_eq!(fake.secret_value(&secret).as_deref(), Some("ghs_secret"));
    assert_bot_claim(&share, &secret);
    assert!(hub.repo_status(&repo).is_some_and(|status| status.linked));

    // Not installed: the secret holds no token, and placing goes on.
    let other = dir.path().join("other");
    project_repo(&other, Some("https://github.com/acme/other.git"));
    let share = hub
        .share_repo(&request_in(&other), &BootReporter::none())
        .await
        .expect("share");
    let secret = share.secret.expect("secret");
    assert_eq!(
        fake.secret_value(&secret).as_deref(),
        Some(crate::repos::UNLINKED)
    );
    let status = hub
        .repo_status(share.repo.as_ref().expect("repo"))
        .expect("status");
    assert!(!status.linked);
    assert_eq!(
        status.install_url.as_deref(),
        Some("https://github.com/apps/aura-agents/installations/new")
    );

    // No origin, or one off GitHub: no secret, and still a branch.
    for (name, origin) in [
        ("local", None),
        ("gitlab", Some("https://gitlab.com/acme/proj.git")),
    ] {
        let project = dir.path().join(name);
        project_repo(&project, origin);
        let share = hub
            .share_repo(&request_in(&project), &BootReporter::none())
            .await
            .expect("share");
        assert!(share.repo.is_none() && share.secret.is_none(), "{name}");
        let claim = CloudHub::git_claim(Some("aura/scout-sc01"), None, None, None).expect("claim");
        assert_eq!((claim.remote_url, claim.env), (None, None));
    }
    assert_eq!(CloudHub::git_claim(None, Some(&repo), None, None), None);
}

/// A hub whose GitHub App is installed on `acme/proj`.
fn github_hub() -> (Arc<CloudHub>, FakeProvider, tempfile::TempDir, RepoRef) {
    let (hub, fake, dir) = hub();
    let tokens = Arc::new(crate::github::FakeRepoTokens::new("aura-agents"));
    let repo = RepoRef {
        owner: "acme".into(),
        name: "proj".into(),
    };
    tokens.install(&repo, "ghs_secret", now_ms() + 3_600_000);
    hub.set_repo_tokens(Some(tokens));
    (hub, fake, dir, repo)
}

/// The node is told the branch, the repository and the bot it commits as.
fn assert_bot_claim(share: &RepoShare, secret: &str) {
    let claim = CloudHub::git_claim(
        Some("aura/scout-sc01"),
        share.repo.as_ref(),
        share.secret.clone(),
        share.identity.as_ref(),
    )
    .expect("claim");
    assert_eq!(claim.branch, "aura/scout-sc01");
    assert_eq!(
        claim.remote_url.as_deref(),
        Some("https://github.com/acme/proj.git")
    );
    assert_eq!(claim.env.as_deref(), Some(secret));
    assert_eq!(claim.placeholder, None);
    assert_eq!(claim.author_name.as_deref(), Some("aura-agents[bot]"));
    assert_eq!(
        claim.author_email.as_deref(),
        Some("41000000+aura-agents[bot]@users.noreply.github.com")
    );
}

/// A project and its agent's machine, whose branch has two commits the
/// project lacks.
struct BranchFixture {
    hub: Arc<CloudHub>,
    _dir: tempfile::TempDir,
    project: PathBuf,
    machine: PathBuf,
    branch: &'static str,
    tip: String,
    agent: AgentId,
    placed: PlacedAgent,
}

async fn branch_fixture() -> BranchFixture {
    let (hub, fake, dir) = hub();
    let project = dir.path().join("project");
    project_repo(&project, None);
    let root = dir.path().join("vm");
    std::fs::create_dir_all(&root).expect("mkdir");
    let machine = root.join("workspace");
    test_git(
        dir.path(),
        &[
            "clone",
            "-q",
            &project.display().to_string(),
            &machine.display().to_string(),
        ],
    );
    test_git(&machine, &["update-ref", workspace::HUB_HEAD_REF, "HEAD"]);
    let branch = "aura/scout-sc01";
    test_git(&machine, &["checkout", "-q", "-b", branch]);
    test_commit(&machine, "b.txt", "agent work");
    let tip = test_commit(&machine, "c.txt", "more agent work");
    run_scripts_in(&fake, &root, &machine);
    let agent = AgentId::new([1; 32]);
    let placed = adopt_machine(&hub, &fake, agent, &project, Some(branch)).await;
    BranchFixture {
        hub,
        _dir: dir,
        project,
        machine,
        branch,
        tip,
        agent,
        placed,
    }
}

#[tokio::test]
async fn the_agents_commits_come_back_on_its_branch_once_each() {
    let BranchFixture {
        hub,
        _dir,
        project,
        machine,
        branch,
        tip,
        agent,
        placed,
    } = branch_fixture().await;
    let hex = agent.to_hex();

    assert_eq!(
        hub.fetch_commits(&hex, &placed).await,
        CommitsOutcome::Fetched {
            branch: branch.into(),
            commits: 2
        }
    );
    assert_eq!(
        test_git(&project, &["rev-parse", &format!("refs/heads/{branch}")]),
        tip
    );
    // The project's own branch and files are left as they were.
    assert_eq!(test_git(&project, &["branch", "--show-current"]), "main");
    assert!(!project.join("b.txt").exists());

    // What came is remembered: nothing new comes again.
    let placed = hub.placed_agent(agent).expect("placed");
    assert_eq!(placed.fetched_tip.as_deref(), Some(tip.as_str()));
    assert_eq!(hub.fetch_commits(&hex, &placed).await, CommitsOutcome::None);
    test_commit(&machine, "d.txt", "last");
    assert_eq!(
        hub.fetch_commits(&hex, &placed).await,
        CommitsOutcome::Fetched {
            branch: branch.into(),
            commits: 1
        }
    );

    // A branch checked out here is not moved under the user.
    test_git(&project, &["checkout", "-q", branch]);
    test_commit(&machine, "e.txt", "after the checkout");
    let placed = hub.placed_agent(agent).expect("placed");
    let CommitsOutcome::Kept { reason, .. } = hub.fetch_commits(&hex, &placed).await else {
        panic!("fetched into the checked-out branch");
    };
    assert!(reason.contains("checked out here"), "{reason}");

    // A placement from before agent branches has none to fetch.
    let legacy = PlacedAgent {
        branch: None,
        ..placed
    };
    assert_eq!(hub.fetch_commits(&hex, &legacy).await, CommitsOutcome::None);
}
