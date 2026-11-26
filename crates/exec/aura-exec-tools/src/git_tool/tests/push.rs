use super::*;

#[tokio::test]
async fn git_push_without_origin_says_so() {
    if !git_available() {
        eprintln!("skip: git not available on PATH");
        return;
    }
    let dir = TempDir::new().unwrap();
    init_repo(dir.path()).await;
    std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
    git_commit_impl(dir.path(), "seed", Duration::from_secs(10))
        .await
        .expect("seed commit");
    let err = git_push_impl(
        dir.path(),
        to_branch("main"),
        PushPolicy::single(Duration::from_secs(5)),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, GitToolError::NoOrigin), "got {err:?}");
}

#[tokio::test]
async fn git_push_to_a_bare_origin_pushes_the_current_branch_and_tracks_it() {
    if !git_available() {
        eprintln!("skip: git not available on PATH");
        return;
    }
    let (_parent, repo, origin) = repo_with_bare_origin().await;
    let pushed = git_push_impl(
        &repo,
        PushTarget::default(),
        PushPolicy::single(Duration::from_secs(20)),
    )
    .await
    .expect("push");
    assert_eq!(pushed.branch, "work");
    assert_eq!(pushed.commits.len(), 1, "{pushed:?}");
    assert_eq!(pushed.commits[0].message, "seed");
    let head = git(&repo, &["rev-parse", "HEAD"]).await;
    assert_eq!(git(&origin, &["rev-parse", "refs/heads/work"]).await, head);
    assert_eq!(
        git(&repo, &["rev-parse", "--abbrev-ref", "work@{upstream}"]).await,
        "origin/work",
        "pushing the current branch makes it track origin"
    );

    // Nothing new: the next push lists no commits.
    let again = git_push_impl(
        &repo,
        PushTarget::default(),
        PushPolicy::single(Duration::from_secs(20)),
    )
    .await
    .expect("push again");
    assert!(again.commits.is_empty(), "{again:?}");

    // Another branch name pushes HEAD there without retargeting `work`.
    git_push_impl(
        &repo,
        to_branch("feature/x"),
        PushPolicy::single(Duration::from_secs(20)),
    )
    .await
    .expect("push to another name");
    assert_eq!(
        git(&origin, &["rev-parse", "refs/heads/feature/x"]).await,
        head
    );
    assert_eq!(
        git(&repo, &["rev-parse", "--abbrev-ref", "work@{upstream}"]).await,
        "origin/work"
    );
}

/// Pushes `repo` with `work` as the only branch allowed.
async fn push_pinned_to_work(
    repo: &Path,
    branch: Option<&str>,
    force: bool,
) -> Result<crate::git_tool::Pushed, GitToolError> {
    let target = PushTarget {
        branch,
        force,
        only_branch: Some("work"),
    };
    git_push_impl(repo, target, PushPolicy::single(Duration::from_secs(20))).await
}

/// Through the tool, the pin comes from the config.
async fn assert_tool_pins_push_branch_from_config(repo: &Path) {
    use crate::sandbox::Sandbox;
    let config = crate::ToolConfig {
        push_branch: Some("work".into()),
        ..Default::default()
    };
    let ctx = ToolContext::new(Sandbox::new(repo).unwrap(), config);
    let err = GitPushTool
        .execute(&ctx, serde_json::json!({ "branch": "main" }))
        .await
        .expect_err("tool refuses other branches");
    assert!(
        matches!(&err, ToolError::InvalidArguments(msg) if msg.contains("work")),
        "got {err:?}"
    );
}

#[tokio::test]
async fn a_pinned_branch_refuses_every_other_branch() {
    if !git_available() {
        eprintln!("skip: git not available on PATH");
        return;
    }
    let (_parent, repo, origin) = repo_with_bare_origin().await;
    for (branch, force) in [(Some("main"), false), (Some("main"), true)] {
        let err = push_pinned_to_work(&repo, branch, force)
            .await
            .expect_err("other branch");
        assert!(
            matches!(&err, GitToolError::BranchNotAllowed { branch, allowed } if branch == "main" && allowed == "work"),
            "got {err:?}"
        );
    }
    // The current branch is checked too: on `other`, a bare push is refused.
    git(&repo, &["checkout", "-q", "-b", "other"]).await;
    let err = push_pinned_to_work(&repo, None, false)
        .await
        .expect_err("current branch is not the pinned one");
    assert!(
        matches!(err, GitToolError::BranchNotAllowed { .. }),
        "{err:?}"
    );
    let refs = git(&origin, &["for-each-ref", "--format=%(refname)"]).await;
    assert!(refs.is_empty(), "nothing reached origin: {refs}");

    // Its own branch, forced or not, goes through.
    push_pinned_to_work(&repo, Some("work"), true)
        .await
        .expect("own branch");
    assert!(!git(&origin, &["rev-parse", "refs/heads/work"])
        .await
        .is_empty());

    assert_tool_pins_push_branch_from_config(&repo).await;
}

#[tokio::test]
async fn commit_push_reports_the_commit_when_the_push_is_refused() {
    if !git_available() {
        eprintln!("skip: git not available on PATH");
        return;
    }
    let (_parent, repo, _origin) = repo_with_bare_origin().await;
    std::fs::write(repo.join("b.txt"), b"b").unwrap();
    let outcome = git_commit_push_impl(
        &repo,
        "more",
        PushTarget {
            branch: Some("main"),
            force: false,
            only_branch: Some("work"),
        },
        Duration::from_secs(10),
        PushPolicy::single(Duration::from_secs(20)),
    )
    .await
    .expect("commit");
    assert!(outcome.commit_sha.is_some());
    assert!(matches!(
        outcome.push_result,
        Err(GitToolError::BranchNotAllowed { .. })
    ));
}

#[tokio::test]
async fn a_detached_head_needs_a_branch() {
    if !git_available() {
        eprintln!("skip: git not available on PATH");
        return;
    }
    let (_parent, repo, _origin) = repo_with_bare_origin().await;
    git(&repo, &["checkout", "-q", "--detach"]).await;
    let err = git_push_impl(
        &repo,
        PushTarget::default(),
        PushPolicy::single(Duration::from_secs(20)),
    )
    .await
    .expect_err("detached");
    assert!(matches!(err, GitToolError::DetachedHead), "{err:?}");
}

#[test]
fn branch_names_that_would_be_options_or_refspecs_are_refused() {
    for good in ["main", "aura/reviewer-re36", "feature/x.y", "v1_2"] {
        assert!(check_branch_name(good).is_ok(), "{good}");
    }
    for bad in [
        "-f",
        "--delete",
        "a:b",
        "HEAD:main",
        "a..b",
        "a b",
        "x.lock",
        "a/",
        "/a",
        "a@{1}",
        "a~1",
        "a^",
        "a\nb",
    ] {
        assert!(check_branch_name(bad).is_err(), "{bad:?}");
    }
    assert!(matches!(
        check_branch_name(""),
        Err(GitToolError::MissingArg("branch"))
    ));
}

#[test]
fn refused_credentials_are_neither_transient_nor_retried() {
    for stderr in [
        "remote: Invalid username or token. Password authentication is not supported.\n\
         fatal: Authentication failed for 'https://github.com/acme/app.git/'",
        "remote: Write access to repository not granted.\n\
         fatal: unable to access 'https://github.com/acme/app.git/': The requested URL returned error: 403",
        "remote: Repository not found.\nfatal: repository 'https://github.com/acme/app.git/' not found",
        "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
        "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.",
    ] {
        assert!(stderr_looks_unauthorized(stderr), "{stderr}");
        assert!(!stderr_looks_transient(stderr), "{stderr}");
    }
    assert!(!stderr_looks_unauthorized("error: RPC failed; result=18"));
    let cloud = GitToolError::Unauthorized {
        remote: "https://github.com/acme/app.git".into(),
        cloud: true,
        stderr: "403".into(),
    };
    assert!(cloud.to_string().contains("GitHub App"), "{cloud}");
    let local = GitToolError::Unauthorized {
        remote: "https://github.com/acme/app.git".into(),
        cloud: false,
        stderr: "403".into(),
    };
    assert!(local.to_string().contains("git credentials"), "{local}");
}

#[tokio::test]
async fn tool_executes_commit_via_context() {
    // This exercises the Tool trait implementation end-to-end.
    if !git_available() {
        eprintln!("skip: git not available on PATH");
        return;
    }
    use crate::sandbox::Sandbox;
    use crate::ToolConfig;

    let dir = TempDir::new().unwrap();
    init_repo(dir.path()).await;
    std::fs::write(dir.path().join("hello.txt"), b"hi").unwrap();

    let sandbox = Sandbox::new(dir.path()).unwrap();
    let mut ctx = ToolContext::new(sandbox, ToolConfig::default());
    ctx.caller_agent_id = Some(aura_core_types::AgentId::generate());
    let result = GitCommitTool
        .execute(&ctx, serde_json::json!({ "message": "e2e" }))
        .await
        .expect("tool should succeed");
    assert!(result.ok);
    let stdout = String::from_utf8_lossy(&result.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["committed"], serde_json::Value::Bool(true));
    assert!(parsed["sha"].as_str().map_or(0, |s| s.len()) == 40);
}
