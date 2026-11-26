//! Unit tests for the `git_tool` module.
//!
//! These tests exercise the internal spawn helpers against a real `git`
//! binary. If `git` is not on PATH the test is skipped (rather than
//! failing) so CI runs in environments without git (docker scratch
//! images, etc.) still pass the rest of the aura-exec-tools suite.

use super::executor::spawn_git;
use super::push::{
    check_branch_name, push_backoff_for_attempt, stderr_looks_remote_exhausted,
    stderr_looks_transient, stderr_looks_unauthorized,
};
use super::redact::redact_url;
use super::{
    git_commit_impl, git_commit_push_impl, git_push_impl, GitCommitPushTool, GitCommitTool,
    GitPushTool, GitToolError, PushPolicy, PushTarget,
};
use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use tokio::process::Command;

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Ends git's search for a repository at `dir`. Git stops at the first
/// `.git` it finds and this one points nowhere, so commands below `dir`
/// fail as "not a git repository" instead of reaching a checkout that
/// holds the temp directory and committing its uncommitted work.
fn fence(dir: &Path) {
    std::fs::write(dir.join(".git"), "gitdir: nowhere\n").unwrap();
}

async fn init_repo(dir: &Path) {
    let status = Command::new("git")
        .arg("init")
        .current_dir(dir)
        .output()
        .await
        .expect("git init");
    assert!(status.status.success(), "git init failed: {status:?}");

    // Local identity so commits are accepted on hosts without a global config.
    for (k, v) in [
        ("user.email", "aura-test@example.com"),
        ("user.name", "Aura Test"),
    ] {
        let s = Command::new("git")
            .args(["config", k, v])
            .current_dir(dir)
            .output()
            .await
            .expect("git config");
        assert!(s.status.success(), "git config {k} failed: {s:?}");
    }
}

async fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .await
        .expect("git");
    assert!(out.status.success(), "git {args:?} failed: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repo on branch `work` with one commit, whose `origin` is a bare
/// repo next to it. Returns (parent dir, repo, bare origin).
async fn repo_with_bare_origin() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let parent = TempDir::new().unwrap();
    let origin = parent.path().join("origin.git");
    let repo = parent.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(parent.path(), &["init", "-q", "--bare", "origin.git"]).await;
    init_repo(&repo).await;
    git(&repo, &["checkout", "-q", "-b", "work"]).await;
    git(
        &repo,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    )
    .await;
    std::fs::write(repo.join("a.txt"), b"a").unwrap();
    git_commit_impl(&repo, "seed", Duration::from_secs(10))
        .await
        .expect("seed commit");
    (parent, repo, origin)
}

fn to_branch(branch: &str) -> PushTarget<'_> {
    PushTarget {
        branch: Some(branch),
        ..PushTarget::default()
    }
}

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

#[tokio::test]
async fn a_pinned_branch_refuses_every_other_branch() {
    if !git_available() {
        eprintln!("skip: git not available on PATH");
        return;
    }
    let (_parent, repo, origin) = repo_with_bare_origin().await;
    let pinned = |branch: Option<&'static str>, force: bool| PushTarget {
        branch,
        force,
        only_branch: Some("work"),
    };
    for (branch, force) in [(Some("main"), false), (Some("main"), true)] {
        let err = git_push_impl(
            &repo,
            pinned(branch, force),
            PushPolicy::single(Duration::from_secs(20)),
        )
        .await
        .expect_err("other branch");
        assert!(
            matches!(&err, GitToolError::BranchNotAllowed { branch, allowed } if branch == "main" && allowed == "work"),
            "got {err:?}"
        );
    }
    // The current branch is checked too: on `other`, a bare push is refused.
    git(&repo, &["checkout", "-q", "-b", "other"]).await;
    let err = git_push_impl(
        &repo,
        pinned(None, false),
        PushPolicy::single(Duration::from_secs(20)),
    )
    .await
    .expect_err("current branch is not the pinned one");
    assert!(
        matches!(err, GitToolError::BranchNotAllowed { .. }),
        "{err:?}"
    );
    let refs = git(&origin, &["for-each-ref", "--format=%(refname)"]).await;
    assert!(refs.is_empty(), "nothing reached origin: {refs}");

    // Its own branch, forced or not, goes through.
    git_push_impl(
        &repo,
        pinned(Some("work"), true),
        PushPolicy::single(Duration::from_secs(20)),
    )
    .await
    .expect("own branch");
    assert!(!git(&origin, &["rev-parse", "refs/heads/work"])
        .await
        .is_empty());

    // Through the tool, the pin comes from the config.
    use crate::sandbox::Sandbox;
    let config = crate::ToolConfig {
        push_branch: Some("work".into()),
        ..Default::default()
    };
    let ctx = ToolContext::new(Sandbox::new(&repo).unwrap(), config);
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

#[test]
fn push_policy_from_config_reads_knobs() {
    let cfg = crate::ToolConfig {
        git_push_timeout_ms: 45_000,
        git_push_attempts: 4,
        ..Default::default()
    };
    let policy = PushPolicy::from_config(&cfg);
    assert_eq!(policy.per_attempt_timeout, Duration::from_millis(45_000));
    assert_eq!(policy.attempts, 4);
}

#[test]
fn push_policy_attempts_clamped_to_at_least_one() {
    let cfg = crate::ToolConfig {
        git_push_attempts: 0,
        ..Default::default()
    };
    let policy = PushPolicy::from_config(&cfg);
    assert_eq!(
        policy.attempts, 1,
        "attempts=0 must be coerced to 1 so the push runs at least once"
    );
}

#[test]
fn stderr_looks_transient_matches_network_markers() {
    assert!(stderr_looks_transient(
        "fatal: unable to access 'https://...': Could not read from remote repository."
    ));
    assert!(stderr_looks_transient("error: RPC failed; result=18"));
    assert!(stderr_looks_transient("early EOF on the wire"));
    assert!(stderr_looks_transient("Connection reset by peer"));
    assert!(stderr_looks_transient("Operation timed out after 30s"));
    // Non-transient failures (auth, non-fast-forward) do NOT retry.
    assert!(!stderr_looks_transient(
        "remote: Permission to foo/bar.git denied to user."
    ));
    assert!(!stderr_looks_transient(
        "! [rejected]   main -> main (non-fast-forward)"
    ));
    assert!(!stderr_looks_transient(
        "fatal: refusing to update checked out branch"
    ));
}

#[test]
fn stderr_looks_remote_exhausted_matches_storage_markers() {
    // Task 2.6 regression: remote storage exhaustion must NOT be
    // classified as transient (retrying cannot heal remote disk
    // within the backoff window) and must be matched by the
    // dedicated short-circuit detector so `git_push_impl` returns
    // `RemoteStorageExhausted` instead of burning the retry budget.
    assert!(stderr_looks_remote_exhausted(
        "remote: fatal: write error: No space left on device"
    ));
    assert!(stderr_looks_remote_exhausted(
        "HTTP 507 Insufficient Storage"
    ));
    assert!(stderr_looks_remote_exhausted("fatal: disk quota exceeded"));
    assert!(stderr_looks_remote_exhausted(
        "remote: fatal: write error: no space left"
    ));
    assert!(!stderr_looks_remote_exhausted("Connection reset by peer"));
    assert!(!stderr_looks_remote_exhausted(
        "remote: Permission to foo/bar.git denied"
    ));
}

#[test]
fn remote_exhaustion_is_not_retried_as_transient() {
    // Retrying a `no space left` push 3x with ~22s backoff is pure
    // latency; the caller must short-circuit so the dev-loop can
    // surface the actionable recovery hint immediately.
    assert!(!stderr_looks_transient(
        "remote: fatal: write error: No space left on device"
    ));
    assert!(!stderr_looks_transient("HTTP 507 Insufficient Storage"));
    assert!(!stderr_looks_transient("fatal: disk quota exceeded"));
    // Even when both a network-ish marker AND a storage marker show
    // up in the same stderr blob, storage wins: the underlying
    // problem is remote disk.
    assert!(!stderr_looks_transient(
        "remote: error: RPC failed; result=18; write error: no space left on device"
    ));
}

#[test]
fn transient_network_markers_still_retry_after_split() {
    // Defence-in-depth: the split between transient and
    // remote-exhausted markers must not demote network failures.
    assert!(stderr_looks_transient("error: RPC failed; result=18"));
    assert!(stderr_looks_transient("Connection reset by peer"));
    assert!(stderr_looks_transient(
        "remote: error: unpack failed: index-pack abnormal exit"
    ));
}

#[test]
fn remote_storage_exhausted_error_renders_actionable_message() {
    let err = GitToolError::RemoteStorageExhausted {
        op: "push",
        stderr: "remote: fatal: write error: No space left on device".into(),
    };
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("remote storage exhausted"),
        "error message must name the failure mode, got: {msg}"
    );
    assert!(
        msg.contains("free space") || msg.contains("switch remotes"),
        "error message must include a recovery hint, got: {msg}"
    );
}

#[test]
fn push_backoff_is_bounded() {
    // "Fail fast, move on" budget: 1s before the first retry,
    // 2s for every retry after that. The default `git_push_attempts`
    // is 2 so production runs only ever wait the first slot, but the
    // cap protects operators who raise the knob.
    assert_eq!(push_backoff_for_attempt(0), Duration::from_secs(1));
    assert_eq!(push_backoff_for_attempt(1), Duration::from_secs(2));
    assert_eq!(push_backoff_for_attempt(2), Duration::from_secs(2));
    // Cap still holds far beyond any reasonable `git_push_attempts`
    // — a misconfigured 10-attempt loop spends ≤19s total in
    // backoff, never minutes.
    assert_eq!(push_backoff_for_attempt(9), Duration::from_secs(2));
}

#[tokio::test]
async fn git_push_retries_on_timeout_then_errors_out() {
    // Unroutable destination + tight per-attempt timeout forces every
    // attempt to time out. We verify the final error is a Timeout and
    // (implicitly) that the retry loop ran all attempts — any early
    // return would have surfaced the first attempt's error, but the
    // test documents the public shape: the function returns a Timeout
    // error even after exhausting retries, rather than bubbling a
    // different variant.
    if !git_available() {
        eprintln!("skip: git not available on PATH");
        return;
    }
    let dir = TempDir::new().unwrap();
    init_repo(dir.path()).await;
    std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
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

    let policy = PushPolicy {
        per_attempt_timeout: Duration::from_millis(50),
        // Two attempts keeps the test fast (one ~1s backoff slot
        // between them under the current schedule) while still
        // exercising the retry path.
        attempts: 2,
    };
    let start = std::time::Instant::now();
    let err = git_push_impl(dir.path(), to_branch("main"), policy)
        .await
        .expect_err("push must time out on all attempts");
    assert!(matches!(err, GitToolError::Timeout(_, _)), "got {err:?}");
    // At least one ~1s backoff slot must have elapsed between the
    // two attempts, confirming the retry loop ran instead of
    // short-circuiting after the first timeout. (The slot is
    // `push_backoff_for_attempt(0)` = 1s; we leave a small slack
    // below the nominal to avoid CI clock-jitter flakes.)
    assert!(
        start.elapsed() >= Duration::from_millis(900),
        "expected retry backoff to add ≥~1s, got {:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn tool_rejects_workspace_escape_via_config() {
    // The tool is hard-wired to use `ctx.sandbox.root()` as its cwd; no
    // caller-supplied `cwd` argument is honored. We assert by placing
    // a non-repo temporary directory as the sandbox root and a git
    // repo *next* to it. The tool must fail (no repo at root), not
    // silently descend into the sibling repo.
    if !git_available() {
        eprintln!("skip: git not available on PATH");
        return;
    }
    use crate::sandbox::Sandbox;
    use crate::ToolConfig;

    let parent = TempDir::new().unwrap();
    fence(parent.path());
    let escape_target = parent.path().join("escape");
    std::fs::create_dir(&escape_target).unwrap();
    init_repo(&escape_target).await;
    std::fs::write(escape_target.join("secret.txt"), b"leaky").unwrap();

    let sandbox_root = parent.path().join("inside");
    std::fs::create_dir(&sandbox_root).unwrap();
    // sandbox_root is NOT a git repo, so git add -A must fail even if
    // the args attempted to redirect execution.
    let sandbox = Sandbox::new(&sandbox_root).unwrap();
    let ctx = ToolContext::new(sandbox, ToolConfig::default());

    // Attempt to slip in a cwd/workspace override via args — ignored
    // by the tool (it uses sandbox.root() exclusively).
    let err = GitCommitTool
        .execute(
            &ctx,
            serde_json::json!({
                "message": "escape attempt",
                "cwd": escape_target.to_string_lossy(),
                "workspace": escape_target.to_string_lossy()
            }),
        )
        .await
        .expect_err("non-repo root must fail");
    // We expect a CommandFailed (git add -A exits non-zero outside a repo).
    assert!(matches!(err, ToolError::CommandFailed(_)), "got {err:?}");
}
