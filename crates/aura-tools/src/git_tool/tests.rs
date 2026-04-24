//! Unit tests for the `git_tool` module.
//!
//! These tests exercise the internal spawn helpers against a real `git`
//! binary. If `git` is not on PATH the test is skipped (rather than
//! failing) so CI runs in environments without git (docker scratch
//! images, etc.) still pass the rest of the aura-tools suite.

use super::*;
use std::path::Path;
use tempfile::TempDir;

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
    let dir = TempDir::new().unwrap();
    let err = git_commit_impl(dir.path(), "should fail", Duration::from_secs(10))
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
    let err = git_push_impl(
        dir.path(),
        "https://10.255.255.1/unreachable.git",
        "main",
        "fake-token",
        false,
        PushPolicy::single(Duration::from_millis(50)),
    )
    .await
    .expect_err("push must time out");
    assert!(matches!(err, GitToolError::Timeout(_, _)), "got {err:?}");
}

#[test]
fn build_auth_url_injects_token() {
    let url = build_auth_url("https://orbit.example.com/acme/repo.git", "jwt123").unwrap();
    assert_eq!(
        url,
        "https://x-token:jwt123@orbit.example.com/acme/repo.git"
    );
}

#[test]
fn build_auth_url_strips_existing_credentials() {
    let url = build_auth_url("https://olduser:oldpass@orbit.example.com/r.git", "new").unwrap();
    assert_eq!(url, "https://x-token:new@orbit.example.com/r.git");
}

#[test]
fn build_auth_url_rejects_non_http_schemes() {
    let err = build_auth_url("ssh://git@github.com/aura/repo.git", "jwt").unwrap_err();
    assert!(matches!(err, GitToolError::InvalidUrl(_)));
}

#[test]
fn build_auth_url_rejects_control_chars_in_token() {
    let err = build_auth_url("https://orbit/r.git", "abc def").unwrap_err();
    assert!(matches!(err, GitToolError::InvalidUrl(_)));
    let err = build_auth_url("https://orbit/r.git", "abc\ndef").unwrap_err();
    assert!(matches!(err, GitToolError::InvalidUrl(_)));
}

#[test]
fn redact_url_masks_user_info() {
    assert_eq!(
        redact_url("https://x-token:secret@orbit.example.com/a/b.git"),
        "https://***@orbit.example.com/a/b.git"
    );
    assert_eq!(
        redact_url("https://orbit.example.com/a/b.git"),
        "https://orbit.example.com/a/b.git"
    );
}

#[tokio::test]
async fn git_push_rejects_missing_fields() {
    let dir = TempDir::new().unwrap();
    let err = git_push_impl(
        dir.path(),
        "",
        "main",
        "jwt",
        false,
        PushPolicy::single(Duration::from_secs(5)),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, GitToolError::MissingArg("remote_url")));
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
    ctx.caller_agent_id = Some(aura_core::AgentId::generate());
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
    let mut cfg = crate::ToolConfig::default();
    cfg.git_push_timeout_ms = 45_000;
    cfg.git_push_attempts = 4;
    let policy = PushPolicy::from_config(&cfg);
    assert_eq!(policy.per_attempt_timeout, Duration::from_millis(45_000));
    assert_eq!(policy.attempts, 4);
}

#[test]
fn push_policy_attempts_clamped_to_at_least_one() {
    let mut cfg = crate::ToolConfig::default();
    cfg.git_push_attempts = 0;
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
    assert_eq!(push_backoff_for_attempt(0), Duration::from_secs(2));
    assert_eq!(push_backoff_for_attempt(1), Duration::from_secs(5));
    assert_eq!(push_backoff_for_attempt(2), Duration::from_secs(15));
    // Cap holds for anything past the third retry so a
    // misconfigured `git_push_attempts` value can't stall the
    // dev-loop for minutes.
    assert_eq!(push_backoff_for_attempt(9), Duration::from_secs(15));
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

    let policy = PushPolicy {
        per_attempt_timeout: Duration::from_millis(50),
        // Two attempts keeps the test fast (~2s backoff between) while
        // still exercising the retry path.
        attempts: 2,
    };
    let start = std::time::Instant::now();
    let err = git_push_impl(
        dir.path(),
        "https://10.255.255.1/unreachable.git",
        "main",
        "fake-token",
        false,
        policy,
    )
    .await
    .expect_err("push must time out on all attempts");
    assert!(matches!(err, GitToolError::Timeout(_, _)), "got {err:?}");
    // At least one 2s backoff must have elapsed between the two
    // attempts, confirming the retry loop ran instead of short-
    // circuiting after the first timeout.
    assert!(
        start.elapsed() >= Duration::from_millis(1_500),
        "expected retry backoff to add ≥1.5s, got {:?}",
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
