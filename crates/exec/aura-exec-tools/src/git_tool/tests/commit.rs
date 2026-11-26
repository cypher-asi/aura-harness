use super::*;

#[tokio::test]
async fn commit_reports_sha_when_there_are_changes() {
    if !git_available() {
        eprintln!("skip: git not available on PATH");
        return;
    }
    let dir = TempDir::new().unwrap();
    init_repo(dir.path()).await;
    std::fs::write(dir.path().join("hello.txt"), b"hello").unwrap();

    let sha = git_commit_impl(dir.path(), "test commit", Duration::from_secs(15))
        .await
        .expect("git_commit_impl");
    let sha = sha.expect("commit should have produced a sha");
    assert_eq!(sha.len(), 40, "expected a 40-char sha, got {sha:?}");
}

#[tokio::test]
async fn commit_returns_none_when_tree_is_clean() {
    if !git_available() {
        eprintln!("skip: git not available on PATH");
        return;
    }
    let dir = TempDir::new().unwrap();
    init_repo(dir.path()).await;
    // No changes staged: commit must be a no-op.
    let sha = git_commit_impl(dir.path(), "nothing to do", Duration::from_secs(15))
        .await
        .expect("git_commit_impl");
    assert!(sha.is_none(), "expected no commit for a clean tree");
}

#[tokio::test]
async fn commit_rejects_empty_message() {
    let dir = TempDir::new().unwrap();
    let err = git_commit_impl(dir.path(), "   ", Duration::from_secs(5))
        .await
        .expect_err("empty message should be rejected");
    assert!(matches!(
        err,
        GitToolError::InvalidArg {
            name: "message",
            ..
        }
    ));
}

#[tokio::test]
async fn commit_surfaces_nonzero_exit_from_add() {
    if !git_available() {
        eprintln!("skip: git not available on PATH");
        return;
    }
    // A non-git directory: `git add -A` fails with a non-zero exit
    // that we must surface as NonZeroExit.
    let parent = TempDir::new().unwrap();
    fence(parent.path());
    let dir = parent.path().join("work");
    std::fs::create_dir(&dir).unwrap();
    let err = git_commit_impl(&dir, "should fail", Duration::from_secs(10))
        .await
        .expect_err("non-repo should fail");
    match err {
        GitToolError::NonZeroExit { op, .. } => assert_eq!(op, "add"),
        other => panic!("expected NonZeroExit on add, got {other:?}"),
    }
}

#[tokio::test]
async fn a_workspace_inside_another_repo_fails_without_touching_it() {
    if !git_available() {
        eprintln!("skip: git not available on PATH");
        return;
    }
    let outer = TempDir::new().unwrap();
    init_repo(outer.path()).await;
    std::fs::write(outer.path().join("theirs.txt"), b"theirs").unwrap();
    let workspace = outer.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("mine.txt"), b"mine").unwrap();

    let err = git_commit_impl(&workspace, "should fail", Duration::from_secs(10))
        .await
        .expect_err("a workspace that isn't a repo's top must not commit");
    assert!(
        matches!(err, GitToolError::NonZeroExit { op: "add", .. }),
        "got {err:?}"
    );
    let err = git_push_impl(
        &workspace,
        PushTarget::default(),
        PushPolicy::single(Duration::from_secs(5)),
    )
    .await
    .expect_err("nor push");
    assert!(
        matches!(err, GitToolError::NonZeroExit { .. }),
        "got {err:?}"
    );

    assert_eq!(
        git(outer.path(), &["rev-list", "--all", "--count"]).await,
        "0"
    );
    assert_eq!(
        git(outer.path(), &["status", "--porcelain"]).await,
        "?? theirs.txt\n?? workspace/",
        "nothing of the outer repo was staged"
    );
}

#[tokio::test]
async fn a_linked_worktree_still_commits_to_its_branch() {
    if !git_available() {
        eprintln!("skip: git not available on PATH");
        return;
    }
    let (parent, repo, _origin) = repo_with_bare_origin().await;
    let worktree = parent.path().join("worktree");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            &worktree.to_string_lossy(),
        ],
    )
    .await;
    std::fs::write(worktree.join("b.txt"), b"b").unwrap();

    let sha = git_commit_impl(&worktree, "from the worktree", Duration::from_secs(10))
        .await
        .expect("commit in a linked worktree")
        .expect("there was a change to commit");
    assert_eq!(git(&repo, &["rev-parse", "side"]).await, sha);
}

#[tokio::test]
async fn spawn_git_enforces_subcommand_allowlist() {
    let dir = TempDir::new().unwrap();
    let err = spawn_git(
        dir.path(),
        "clone",
        &["https://example.com/x.git"],
        Duration::from_secs(1),
        "clone",
    )
    .await
    .expect_err("clone is not on the allow-list");
    assert!(matches!(err, GitToolError::DisallowedSubcommand(ref s) if s == "clone"));
}

#[tokio::test]
async fn spawn_git_times_out() {
    if !git_available() {
        eprintln!("skip: git not available on PATH");
        return;
    }
    // `git push` to an unreachable URL will block on DNS/TCP for well
    // over 50 ms. We cap the timeout aggressively to prove the hard
    // kill path works.
    let dir = TempDir::new().unwrap();
    init_repo(dir.path()).await;
    std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
    // Create a commit so `push` has something to try to send.
    let _ = git_commit_impl(dir.path(), "seed", Duration::from_secs(10))
        .await
        .expect("seed commit");
    git(
        dir.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://10.255.255.1/unreachable.git",
        ],
    )
    .await;
    let err = git_push_impl(
        dir.path(),
        to_branch("main"),
        PushPolicy::single(Duration::from_millis(50)),
    )
    .await
    .expect_err("push must time out");
    assert!(matches!(err, GitToolError::Timeout(_, _)), "got {err:?}");
}

#[test]
fn redact_url_masks_user_info() {
    assert_eq!(
        redact_url("https://x-access-token:secret@github.com/a/b.git"),
        "https://***@github.com/a/b.git"
    );
    assert_eq!(
        redact_url("https://github.com/a/b.git"),
        "https://github.com/a/b.git"
    );
}

#[test]
fn push_schemas_take_no_credentials() {
    for definition in [GitPushTool.definition(), GitCommitPushTool.definition()] {
        let schema = definition.input_schema.to_string();
        for field in ["jwt", "token", "remote_url", "password"] {
            assert!(
                !schema.contains(&format!("\"{field}\"")),
                "{} takes {field}: {schema}",
                definition.name
            );
        }
        assert!(schema.contains("\"branch\""), "{schema}");
    }
    let required = GitCommitPushTool.definition().input_schema["required"].clone();
    assert_eq!(required, serde_json::json!(["message"]));
    assert!(GitPushTool.definition().input_schema["required"].is_null());
}
