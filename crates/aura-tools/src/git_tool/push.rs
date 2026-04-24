//! `git_push_impl` — the underlying implementation behind
//! [`crate::git_tool::GitPushTool`], plus the retry / classification
//! helpers it relies on (transient vs remote-storage exhaustion).
//!
//! Push is its own module because it owns:
//!
//! - The retry loop + exponential backoff schedule.
//! - The transient-error classifier ([`stderr_looks_transient`]).
//! - The remote-storage-exhausted short-circuit
//!   ([`stderr_looks_remote_exhausted`]) introduced in Task 2.6.
//!
//! Everything else (`git add`, `git commit`, `git rev-parse`) is
//! best-served by the simpler [`run_git_expect_ok`] path in
//! [`super::executor`].
//!
//! [`run_git_expect_ok`]: super::executor::run_git_expect_ok

use std::time::Duration;

use tracing::{info, instrument, warn};

use super::executor::{list_unpushed_commits, run_git_expect_ok};
use super::redact::{build_auth_url, redact_url};
use super::{CommitInfo, GitToolError, PushPolicy};

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

pub(super) fn stderr_looks_transient(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    // Storage-exhaustion markers take precedence: a push that
    // reports both `no space left on device` AND `rpc failed` is
    // fundamentally a remote-disk problem, not a network one.
    if REMOTE_EXHAUSTED_PUSH_STDERR
        .iter()
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

/// Bounded exponential backoff between push attempts: 2s, 5s, 15s,
/// then 15s for every subsequent attempt. Keeping the cap low means
/// a push that fails all attempts still returns within ~22s of extra
/// wall-clock — small enough that the dev-loop's post-commit handoff
/// isn't held up for minutes.
pub(super) fn push_backoff_for_attempt(attempt_index: u32) -> Duration {
    match attempt_index {
        0 => Duration::from_secs(2),
        1 => Duration::from_secs(5),
        _ => Duration::from_secs(15),
    }
}

pub(super) fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Push the current `HEAD` to `remote_url:branch` using a JWT-auth URL.
///
/// The push is executed with the fully-authenticated URL passed inline
/// (rather than registering a named remote) so no credentials survive
/// in the on-disk `.git/config`. Returns the list of SHAs that were
/// newly pushed (derived from `git log orbit/branch..HEAD` before the
/// push — best-effort, empty on failure).
///
/// Retries on transient failures (timeouts, `could not read from
/// remote`, `RPC failed`, etc.) up to `policy.attempts` total attempts
/// with exponential backoff. Non-transient failures (auth errors,
/// non-fast-forward without `--force`) short-circuit immediately.
#[instrument(skip_all, fields(op = "push", branch = %branch))]
pub async fn git_push_impl(
    workspace: &std::path::Path,
    remote_url: &str,
    branch: &str,
    jwt: &str,
    force: bool,
    policy: PushPolicy,
) -> Result<Vec<CommitInfo>, GitToolError> {
    if remote_url.is_empty() {
        return Err(GitToolError::MissingArg("remote_url"));
    }
    if branch.is_empty() {
        return Err(GitToolError::MissingArg("branch"));
    }
    if jwt.is_empty() {
        return Err(GitToolError::MissingArg("jwt"));
    }

    let auth_url = build_auth_url(remote_url, jwt)?;
    let safe_remote = redact_url(remote_url);
    let per_attempt_timeout = policy.per_attempt_timeout;
    let attempts = policy.attempts.max(1);

    // Best-effort unpushed commit listing. Missing remote tracking
    // branches are normal for first-push flows; swallow the error.
    let refspec = format!("HEAD:refs/heads/{branch}");
    let commits = list_unpushed_commits(workspace, "HEAD", per_attempt_timeout)
        .await
        .unwrap_or_default();

    let mut args: Vec<&str> = vec![auth_url.as_str()];
    if force {
        args.push("--force");
    }
    args.push(refspec.as_str());

    let mut last_err: Option<GitToolError> = None;
    for attempt in 0..attempts {
        match run_git_expect_ok(workspace, "push", &args, per_attempt_timeout, "push").await {
            Ok(_) => {
                info!(
                    remote = %safe_remote,
                    %branch,
                    commit_count = commits.len(),
                    attempt = attempt + 1,
                    "git push succeeded"
                );
                return Ok(commits);
            }
            Err(err) => {
                // Remote storage exhaustion: short-circuit with a
                // dedicated variant so the dev-loop renders an
                // actionable recovery hint. Retrying cannot heal
                // remote disk within our backoff window.
                if let GitToolError::NonZeroExit { stderr, .. } = &err {
                    if stderr_looks_remote_exhausted(stderr) {
                        warn!(
                            remote = %safe_remote,
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
                }
                let transient = match &err {
                    GitToolError::Timeout(_, _) => true,
                    GitToolError::NonZeroExit { stderr, .. } => stderr_looks_transient(stderr),
                    // Spawn / URL / arg errors never retry — they are
                    // deterministic misconfigurations.
                    _ => false,
                };
                let attempts_left = attempts - (attempt + 1);
                if !transient || attempts_left == 0 {
                    if transient {
                        warn!(
                            remote = %safe_remote,
                            %branch,
                            attempt = attempt + 1,
                            error = %err,
                            "git push failed after exhausting retry budget"
                        );
                    } else {
                        warn!(
                            remote = %safe_remote,
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
                    remote = %safe_remote,
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
