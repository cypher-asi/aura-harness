//! `git_push_impl` — the underlying implementation behind
//! [`crate::git_tool::GitPushTool`], plus the retry / classification
//! helpers it relies on.
//!
//! Push is its own module because it owns:
//!
//! - Resolving and checking the target branch ([`PushTarget`]).
//! - The retry loop + bounded backoff schedule.
//! - The failure classifiers: rejected credentials
//!   ([`stderr_looks_unauthorized`]), remote storage exhaustion
//!   ([`stderr_looks_remote_exhausted`]) and transient network errors
//!   ([`stderr_looks_transient`]).
//!
//! Everything else (`git add`, `git commit`, `git rev-parse`) is
//! best-served by the simpler [`run_git_expect_ok`] path in
//! [`super::executor`].
//!
//! [`run_git_expect_ok`]: super::executor::run_git_expect_ok

use std::time::Duration;

use tracing::{info, instrument, warn};

use super::executor::{list_unpushed_commits, run_git_expect_ok};
use super::redact::redact_url;
use super::{GitToolError, PushPolicy, PushTarget, Pushed};

/// The remote every push goes to.
pub(super) const REMOTE: &str = "origin";

/// Limit on the local reads before a push (`rev-parse`, `remote
/// get-url`, `log`).
const INSPECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Transient git push stderr markers that warrant a retry. Non-transient
/// failures (auth rejected, non-fast-forward without `--force`, malformed
/// refspec) surface immediately so the agent gets the signal it needs
/// without spending its retry budget on a dead-letter push.
const TRANSIENT_PUSH_STDERR: &[&str] = &[
    "could not read from remote",
    "fatal: unable to access",
    "rpc failed",
    "early eof",
    "connection reset",
    "broken pipe",
    "operation timed out",
    "unable to connect",
    "temporary failure in name resolution",
    "ssl_read",
    "tls",
    // Server-side unpack/index failures can have transient roots
    // (receive-pack sigterm, concurrent writes), so retry them even
    // though they can also co-occur with remote-storage exhaustion —
    // we check the storage-exhaustion list first.
    "unpack failed",
    "index-pack abnormal exit",
];

/// Remote-side storage exhaustion markers. Retrying these within the
/// dev-loop's backoff window cannot heal remote disk; short-circuit
/// instead so the caller can render an actionable recovery hint and
/// the dev-loop does not burn ~22s of backoff per push attempt.
const REMOTE_EXHAUSTED_PUSH_STDERR: &[&str] = &[
    "no space left on device",
    "insufficient storage",
    "http 507",
    "disk quota exceeded",
    "write error: no space",
];

/// The remote refused the credentials, or they grant no access to the
/// repository (GitHub answers 404 for a private repository the token
/// can't see). Checked before the transient list: `fatal: unable to
/// access ...: The requested URL returned error: 403` is not worth a
/// retry.
const UNAUTHORIZED_PUSH_STDERR: &[&str] = &[
    "authentication failed",
    "invalid username or password",
    "invalid username or token",
    "could not read username",
    "could not read password",
    "returned error: 401",
    "returned error: 403",
    "write access to repository not granted",
    "permission denied",
    "repository not found",
    "denied to",
];

pub(super) fn stderr_looks_transient(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    // Storage-exhaustion and credential markers take precedence: a
    // push that reports both `no space left on device` AND `rpc
    // failed` is fundamentally a remote-disk problem, not a network
    // one.
    if REMOTE_EXHAUSTED_PUSH_STDERR
        .iter()
        .chain(UNAUTHORIZED_PUSH_STDERR)
        .any(|m| lower.contains(m))
    {
        return false;
    }
    TRANSIENT_PUSH_STDERR.iter().any(|m| lower.contains(m))
}

pub(super) fn stderr_looks_remote_exhausted(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    REMOTE_EXHAUSTED_PUSH_STDERR
        .iter()
        .any(|m| lower.contains(m))
}

pub(super) fn stderr_looks_unauthorized(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    UNAUTHORIZED_PUSH_STDERR.iter().any(|m| lower.contains(m))
}

/// Bounded backoff between push attempts: 1s, 2s, then 2s for every
/// subsequent attempt.
///
/// The default `ToolConfig` only runs two attempts total, so most of
/// this schedule is academic — the only inter-attempt gap an
/// out-of-the-box dev-loop ever waits is the first 1s. The cap stays
/// at 2s (not 15s like the previous schedule) so a misconfigured
/// `git_push_attempts` of 5+ still cannot stall the agent for more
/// than a few extra seconds; "fail fast, move on" is the explicit
/// policy here.
pub(super) fn push_backoff_for_attempt(attempt_index: u32) -> Duration {
    match attempt_index {
        0 => Duration::from_secs(1),
        _ => Duration::from_secs(2),
    }
}

pub(super) fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Refuse branch names that are not plain branch names: a leading `-`
/// would read as an option and a `:` would turn the refspec into
/// another one.
pub(super) fn check_branch_name(branch: &str) -> Result<(), GitToolError> {
    let invalid = |reason: &str| GitToolError::InvalidArg {
        name: "branch",
        reason: format!("{branch:?} {reason}"),
    };
    if branch.is_empty() {
        return Err(GitToolError::MissingArg("branch"));
    }
    if branch.starts_with('-') || branch.starts_with('/') || branch.ends_with('/') {
        return Err(invalid("must not start with '-' or '/', or end with '/'"));
    }
    if branch.ends_with(".lock") || branch.ends_with('.') || branch.contains("..") {
        return Err(invalid("is not a valid branch name"));
    }
    if branch.contains("@{") || branch.contains("//") {
        return Err(invalid("is not a valid branch name"));
    }
    if branch.chars().any(|c| {
        c.is_ascii_whitespace()
            || c.is_control()
            || matches!(c, ':' | '~' | '^' | '?' | '*' | '[' | '\\')
    }) {
        return Err(invalid("holds a character branch names can't"));
    }
    Ok(())
}

/// The branch `HEAD` is on, or `None` when it is detached.
async fn current_branch(
    workspace: &std::path::Path,
    timeout: Duration,
) -> Result<Option<String>, GitToolError> {
    let out = run_git_expect_ok(
        workspace,
        "rev-parse",
        &["--abbrev-ref", "HEAD"],
        timeout,
        "rev-parse --abbrev-ref HEAD",
    )
    .await?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((!name.is_empty() && name != "HEAD").then_some(name))
}

/// The URL of [`REMOTE`], with any user-info masked.
async fn remote_url(
    workspace: &std::path::Path,
    timeout: Duration,
) -> Result<String, GitToolError> {
    match run_git_expect_ok(
        workspace,
        "remote",
        &["get-url", REMOTE],
        timeout,
        "remote get-url",
    )
    .await
    {
        Ok(out) => Ok(redact_url(String::from_utf8_lossy(&out.stdout).trim())),
        Err(GitToolError::NonZeroExit { .. }) => Err(GitToolError::NoOrigin),
        Err(err) => Err(err),
    }
}

/// Push the current `HEAD` to `origin`, authenticating with git's own
/// credentials (a credential helper or SSH key here; on a cloud machine
/// the credential store its claim wrote). No credential is ever taken
/// as an argument or put on the command line.
///
/// `target.branch` defaults to the current branch; `target.only_branch`
/// refuses every other branch. Pushing the current branch to its own
/// name also sets it to track `origin` (`--set-upstream`), so `git
/// status` and `gh pr create` find it.
///
/// Returns the remote, the branch and the commits `origin` didn't have
/// as far as this repository knows (best-effort, at most 50).
///
/// Retries on transient failures (timeouts, `could not read from
/// remote`, `RPC failed`, etc.) up to `policy.attempts` total attempts
/// with a bounded backoff. Non-transient failures (rejected
/// credentials, non-fast-forward without `--force`) short-circuit
/// immediately.
#[instrument(skip_all, fields(op = "push"))]
pub async fn git_push_impl(
    workspace: &std::path::Path,
    target: PushTarget<'_>,
    policy: PushPolicy,
) -> Result<Pushed, GitToolError> {
    let per_attempt_timeout = policy.per_attempt_timeout;
    let attempts = policy.attempts.max(1);

    let current = current_branch(workspace, INSPECT_TIMEOUT).await?;
    let branch = match (target.branch, current.as_deref()) {
        (Some(branch), _) => branch.to_string(),
        (None, Some(current)) => current.to_string(),
        (None, None) => return Err(GitToolError::DetachedHead),
    };
    check_branch_name(&branch)?;
    if let Some(only) = target.only_branch {
        if branch != only {
            return Err(GitToolError::BranchNotAllowed {
                branch,
                allowed: only.to_string(),
            });
        }
    }
    let remote = remote_url(workspace, INSPECT_TIMEOUT).await?;

    // Best-effort unpushed commit listing: a repository that never
    // fetched from `origin` lists its recent history instead.
    let commits = list_unpushed_commits(workspace, REMOTE, INSPECT_TIMEOUT)
        .await
        .unwrap_or_default();

    let refspec = format!("HEAD:refs/heads/{branch}");
    let mut args: Vec<&str> = Vec::new();
    if current.as_deref() == Some(branch.as_str()) {
        args.push("--set-upstream");
    }
    if target.force {
        args.push("--force");
    }
    args.push(REMOTE);
    args.push(refspec.as_str());

    let mut last_err: Option<GitToolError> = None;
    for attempt in 0..attempts {
        match run_git_expect_ok(workspace, "push", &args, per_attempt_timeout, "push").await {
            Ok(_) => {
                info!(
                    %remote,
                    %branch,
                    commit_count = commits.len(),
                    attempt = attempt + 1,
                    "git push succeeded"
                );
                return Ok(Pushed {
                    remote,
                    branch,
                    commits,
                });
            }
            Err(err) => {
                if let GitToolError::NonZeroExit { stderr, .. } = &err {
                    // Remote storage exhaustion: short-circuit with a
                    // dedicated variant so the dev-loop renders an
                    // actionable recovery hint. Retrying cannot heal
                    // remote disk within our backoff window.
                    if stderr_looks_remote_exhausted(stderr) {
                        warn!(
                            %remote,
                            %branch,
                            attempt = attempt + 1,
                            error = %err,
                            "git push failed: remote storage exhausted; not retrying"
                        );
                        return Err(GitToolError::RemoteStorageExhausted {
                            op: "push",
                            stderr: stderr.clone(),
                        });
                    }
                    if stderr_looks_unauthorized(stderr) {
                        warn!(
                            %remote,
                            %branch,
                            attempt = attempt + 1,
                            error = %err,
                            "git push failed: credentials refused; not retrying"
                        );
                        return Err(GitToolError::Unauthorized {
                            remote,
                            cloud: target.only_branch.is_some(),
                            stderr: stderr.clone(),
                        });
                    }
                }
                let transient = match &err {
                    GitToolError::Timeout(_, _) => true,
                    GitToolError::NonZeroExit { stderr, .. } => stderr_looks_transient(stderr),
                    // Spawn / arg errors never retry — they are
                    // deterministic misconfigurations.
                    _ => false,
                };
                let attempts_left = attempts - (attempt + 1);
                if !transient || attempts_left == 0 {
                    if transient {
                        warn!(
                            %remote,
                            %branch,
                            attempt = attempt + 1,
                            error = %err,
                            "git push failed after exhausting retry budget"
                        );
                    } else {
                        warn!(
                            %remote,
                            %branch,
                            attempt = attempt + 1,
                            error = %err,
                            "git push failed (non-retryable)"
                        );
                    }
                    last_err = Some(err);
                    break;
                }
                let backoff = push_backoff_for_attempt(attempt);
                warn!(
                    %remote,
                    %branch,
                    attempt = attempt + 1,
                    attempts_left,
                    backoff_ms = duration_millis_u64(backoff),
                    error = %err,
                    "git push failed transiently; retrying"
                );
                last_err = Some(err);
                tokio::time::sleep(backoff).await;
            }
        }
    }
    // Infallible in practice — the loop always records an error before
    // breaking — but guard against the empty-loop case where
    // `attempts == 0` was passed (coerced to 1 above, so unreachable).
    Err(last_err.unwrap_or(GitToolError::Timeout("push", per_attempt_timeout)))
}
