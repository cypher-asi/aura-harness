use super::*;

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
    let err = git_push_impl(dir.path(), to_branch("work"), policy)
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
