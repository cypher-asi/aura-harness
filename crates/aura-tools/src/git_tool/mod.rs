//! Git mutation tools (`git_commit`, `git_push`, `git_commit_push`).
//!
//! This module centralises every mutating `git` subprocess invocation
//! behind the kernel-mediated tool pipeline. See `docs/invariants.md` §1
//! (Sole External Gateway): spawning `git` with `add`, `commit`, or
//! `push` outside this module is a bug. Read-only git inspection
//! (`diff`, `status`, `log`) remains an [exception](../../../docs/invariants.md)
//! and lives in `aura-agent/src/git.rs`.
//!
//! Each tool in this module:
//! - Uses `tokio::process::Command` with a hard timeout.
//! - Operates strictly inside `ToolContext::sandbox.root()` — the kernel
//!   rooted workspace. No caller-supplied cwd is honored.
//! - Restricts itself to the allow-listed subcommand (`add`, `commit`,
//!   `push`, plus the read-only helpers `diff --cached` and
//!   `rev-parse HEAD` used to implement commit).
//! - Scrubs credentials from structured tracing output.
//!
//! Tool availability is resolved by the kernel's tri-state policy
//! (`UserToolDefaults` plus optional `AgentToolPermissions`). Even when
//! these tools resolve to `on`, this module still enforces workspace
//! confinement, subcommand allowlists, timeouts, and credential scrubbing.

use std::time::Duration;

use async_trait::async_trait;
use aura_core::{ToolDefinition, ToolResult};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::process::Command;
use tracing::{debug, info, instrument, warn};

use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};

// -----------------------------------------------------------------------
// Error type
// -----------------------------------------------------------------------

/// Errors surfaced from the `git_*` tools.
///
/// Kept as a dedicated `thiserror` enum so `aura-tools` stays free of
/// `anyhow` (per crate rules) and callers can match on specific
/// failure modes without string-matching.
#[derive(Debug, Error)]
pub enum GitToolError {
    #[error("git subcommand '{0}' is not on the allow-list (add, commit, push)")]
    DisallowedSubcommand(String),

    #[error("missing required argument: {0}")]
    MissingArg(&'static str),

    #[error("invalid argument {name}: {reason}")]
    InvalidArg { name: &'static str, reason: String },

    #[error("git {op} failed with exit code {exit_code}: {stderr}")]
    NonZeroExit {
        op: &'static str,
        exit_code: i32,
        stderr: String,
    },

    #[error("git {0} timed out after {1:?}")]
    Timeout(&'static str, Duration),

    #[error("failed to spawn git {0}: {1}")]
    Spawn(&'static str, std::io::Error),

    #[error("invalid remote URL: {0}")]
    InvalidUrl(String),

    /// The remote rejected the push because it could not persist the
    /// objects (out of disk, HTTP 507, disk quota exceeded, etc.).
    /// This is deliberately a non-retryable variant because retrying
    /// within the dev-loop's backoff window cannot heal remote
    /// storage — the operator must free space or redirect the remote
    /// before the next push attempt. Surfaced as a specific variant
    /// so the dev-loop can render an actionable recovery hint instead
    /// of a generic "push failed" message.
    #[error(
        "remote storage exhausted on git {op}; free space on the remote or switch remotes. \
         server reported: {stderr}"
    )]
    RemoteStorageExhausted { op: &'static str, stderr: String },
}

impl From<GitToolError> for ToolError {
    fn from(value: GitToolError) -> Self {
        match value {
            GitToolError::DisallowedSubcommand(s) => Self::CommandNotAllowed(s),
            GitToolError::MissingArg(name) => {
                Self::InvalidArguments(format!("missing required field '{name}'"))
            }
            GitToolError::InvalidArg { name, reason } => {
                Self::InvalidArguments(format!("{name}: {reason}"))
            }
            GitToolError::Timeout(_, d) => Self::CommandTimeout {
                timeout_ms: u64::try_from(d.as_millis()).unwrap_or(u64::MAX),
            },
            GitToolError::Spawn(_, e) => Self::Io(e),
            GitToolError::NonZeroExit {
                op,
                exit_code,
                stderr,
            } => Self::CommandFailed(format!("git {op} exited {exit_code}: {stderr}")),
            GitToolError::InvalidUrl(msg) => Self::InvalidArguments(msg),
            GitToolError::RemoteStorageExhausted { op, stderr } => Self::CommandFailed(format!(
                "git {op} failed: remote storage exhausted ({stderr}). \
                 Free space on the remote (or switch remotes) before retrying."
            )),
        }
    }
}

// -----------------------------------------------------------------------
// Allow-list + internal runner
// -----------------------------------------------------------------------

/// Subcommands the executor is permitted to invoke. Any other value is
/// rejected with [`GitToolError::DisallowedSubcommand`].
pub(crate) const ALLOWED_SUBCOMMANDS: &[&str] =
    &["add", "commit", "push", "diff", "rev-parse", "remote"];

fn ensure_allowed(subcmd: &str) -> Result<(), GitToolError> {
    if ALLOWED_SUBCOMMANDS.contains(&subcmd) {
        Ok(())
    } else {
        Err(GitToolError::DisallowedSubcommand(subcmd.to_string()))
    }
}

/// Spawn `git <subcmd> <args>` under `workspace`, with a hard timeout.
///
/// Returns the raw [`std::process::Output`] on completion. Neither the
/// program (`git`) nor the subcommand set is caller-controlled once
/// this helper is reached — both are fixed by the calling tool.
#[instrument(skip_all, fields(op = %op_label, subcmd = %subcmd))]
pub(crate) async fn spawn_git(
    workspace: &std::path::Path,
    subcmd: &str,
    args: &[&str],
    timeout: Duration,
    op_label: &'static str,
) -> Result<std::process::Output, GitToolError> {
    ensure_allowed(subcmd)?;

    let mut cmd = Command::new("git");
    cmd.arg(subcmd)
        .args(args)
        .current_dir(workspace)
        // Never prompt for credentials — we always pre-embed auth into
        // the URL and rely on the non-interactive push path.
        .env("GIT_TERMINAL_PROMPT", "0");

    debug!(?workspace, "spawning git");

    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| GitToolError::Timeout(op_label, timeout))?
        .map_err(|e| GitToolError::Spawn(op_label, e))?;

    Ok(output)
}

/// Shortcut that converts a non-zero exit into [`GitToolError::NonZeroExit`].
async fn run_git_expect_ok(
    workspace: &std::path::Path,
    subcmd: &str,
    args: &[&str],
    timeout: Duration,
    op_label: &'static str,
) -> Result<std::process::Output, GitToolError> {
    let output = spawn_git(workspace, subcmd, args, timeout, op_label).await?;
    if !output.status.success() {
        return Err(GitToolError::NonZeroExit {
            op: op_label,
            exit_code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(output)
}

// -----------------------------------------------------------------------
// Operation implementations (shared between tools + orbit)
// -----------------------------------------------------------------------

/// Metadata recorded for each commit surfaced by `git_push`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
}

/// Stage every tracked change and create a commit with `message`.
/// Returns `Ok(None)` when there is nothing to commit (`git diff
/// --cached --quiet` exits 0 after staging).
#[instrument(skip_all, fields(op = "commit"))]
pub async fn git_commit_impl(
    workspace: &std::path::Path,
    message: &str,
    timeout: Duration,
) -> Result<Option<String>, GitToolError> {
    if message.trim().is_empty() {
        return Err(GitToolError::InvalidArg {
            name: "message",
            reason: "commit message must not be empty".into(),
        });
    }

    run_git_expect_ok(workspace, "add", &["-A"], timeout, "add").await?;

    let diff = spawn_git(
        workspace,
        "diff",
        &["--cached", "--quiet"],
        timeout,
        "diff --cached",
    )
    .await?;
    if diff.status.success() {
        info!("no staged changes; skipping commit");
        return Ok(None);
    }

    run_git_expect_ok(workspace, "commit", &["-m", message], timeout, "commit").await?;

    let sha_output =
        run_git_expect_ok(workspace, "rev-parse", &["HEAD"], timeout, "rev-parse HEAD").await?;
    let sha = String::from_utf8_lossy(&sha_output.stdout)
        .trim()
        .to_string();
    info!(%sha, "git commit created");
    Ok(Some(sha))
}

/// Per-attempt timeout and attempt budget for `git push`.
///
/// Push is a network operation and benefits from a different budget
/// than the rest of the git tools — Orbit round-trips routinely
/// exceed the 120s cap that covers `git add` / `git commit`. Keeping
/// push on its own knob also makes it safe to retry: a single slow
/// remote shouldn't force us to raise every other tool's ceiling.
#[derive(Debug, Clone, Copy)]
pub struct PushPolicy {
    /// Per-attempt timeout passed to the underlying `git push`
    /// subprocess.
    pub per_attempt_timeout: Duration,
    /// Total number of attempts, including the initial one. Values
    /// below 1 are coerced to 1 at the call-site.
    pub attempts: u32,
}

impl PushPolicy {
    /// Single-attempt policy with `timeout` — preserves the pre-retry
    /// behavior for callers that don't care about the retry surface
    /// (tests, ad-hoc tools).
    pub const fn single(timeout: Duration) -> Self {
        Self {
            per_attempt_timeout: timeout,
            attempts: 1,
        }
    }

    /// Build a push policy from the crate's `ToolConfig` knobs
    /// (`git_push_timeout_ms` + `git_push_attempts`). Any `attempts`
    /// value below 1 is coerced to 1 so the call-site is free to
    /// trust the `attempts` field non-zero.
    pub fn from_config(config: &crate::ToolConfig) -> Self {
        Self {
            per_attempt_timeout: Duration::from_millis(config.git_push_timeout_ms),
            attempts: config.git_push_attempts.max(1),
        }
    }
}

/// Outcome of [`git_commit_push_impl`].
///
/// The commit half and the push half are reported independently so a
/// push-only failure does not mask a successful commit. The orchestrator
/// (dev-loop automaton) uses this to emit `GitCommitted` alongside
/// `GitPushFailed` when the commit landed locally but the push timed
/// out — preserving the commit SHA in the task history instead of
/// pretending the work never happened.
#[derive(Debug)]
pub struct CommitPushOutcome {
    /// Commit SHA from the local `git commit`, or `None` when there
    /// were no staged changes (i.e. the tool was a no-op).
    pub commit_sha: Option<String>,
    /// Result of the subsequent `git push`. `Ok` on success, `Err`
    /// on any push-specific failure (timeout, non-zero exit, spawn
    /// error). Independent of `commit_sha` — the commit SHA stays
    /// populated even when the push fails.
    pub push_result: Result<Vec<CommitInfo>, GitToolError>,
}

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

fn stderr_looks_transient(stderr: &str) -> bool {
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

fn stderr_looks_remote_exhausted(stderr: &str) -> bool {
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
fn push_backoff_for_attempt(attempt_index: u32) -> Duration {
    match attempt_index {
        0 => Duration::from_secs(2),
        1 => Duration::from_secs(5),
        _ => Duration::from_secs(15),
    }
}

fn duration_millis_u64(duration: Duration) -> u64 {
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

/// Combined `add -A` + `commit` + `push` — the transactional group the
/// dev-loop relies on.
///
/// Commit failures propagate via the outer `Result` (nothing landed).
/// Push failures are reported inline via [`CommitPushOutcome::push_result`]
/// so the caller still sees the commit SHA — the task's work is locally
/// persisted even if Orbit wasn't reachable.
#[instrument(skip_all, fields(op = "commit_and_push", branch = %branch))]
#[allow(clippy::too_many_arguments)]
pub async fn git_commit_push_impl(
    workspace: &std::path::Path,
    message: &str,
    remote_url: &str,
    branch: &str,
    jwt: &str,
    force: bool,
    commit_timeout: Duration,
    push_policy: PushPolicy,
) -> Result<CommitPushOutcome, GitToolError> {
    let commit_sha = git_commit_impl(workspace, message, commit_timeout).await?;
    let push_result = git_push_impl(workspace, remote_url, branch, jwt, force, push_policy).await;
    Ok(CommitPushOutcome {
        commit_sha,
        push_result,
    })
}

/// Read `git log HEAD --pretty=format:'%H %s'` up to 50 entries. Only
/// used for audit logging on successful pushes. Missing refs yield an
/// empty vec rather than an error.
async fn list_unpushed_commits(
    workspace: &std::path::Path,
    git_ref: &str,
    timeout: Duration,
) -> Result<Vec<CommitInfo>, GitToolError> {
    // `log` is a read-only subcommand. The allow-list keeps it close
    // to `rev-parse` — both are inspection helpers for the mutating
    // flow and never take caller-controlled args beyond a bounded ref.
    let mut cmd = Command::new("git");
    cmd.arg("log")
        .arg(git_ref)
        .arg("--pretty=format:%H %s")
        .arg("-50")
        .current_dir(workspace)
        .env("GIT_TERMINAL_PROMPT", "0");

    let output = match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(GitToolError::Spawn("log", e)),
        Err(_) => return Err(GitToolError::Timeout("log", timeout)),
    };
    if !output.status.success() {
        debug!(
            stderr = %String::from_utf8_lossy(&output.stderr),
            "git log returned non-zero exit; treating as empty"
        );
        return Ok(Vec::new());
    }

    let commits = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (sha, msg) = line.split_once(' ')?;
            Some(CommitInfo {
                sha: sha.to_string(),
                message: msg.to_string(),
            })
        })
        .collect();
    Ok(commits)
}

/// Inject `x-token:<jwt>` into `remote_url`'s user-info component.
///
/// Accepts `https://host[:port]/path` URLs. Any other shape — including
/// ssh://, file://, scp-style `git@host:path` — is rejected with
/// [`GitToolError::InvalidUrl`]. The caller is responsible for stripping
/// existing credentials; we do not attempt to merge them.
fn build_auth_url(remote_url: &str, jwt: &str) -> Result<String, GitToolError> {
    // Find scheme.
    let Some((scheme, rest)) = remote_url.split_once("://") else {
        return Err(GitToolError::InvalidUrl(
            "remote URL must contain '://'".into(),
        ));
    };
    if scheme != "https" && scheme != "http" {
        return Err(GitToolError::InvalidUrl(format!(
            "unsupported scheme '{scheme}' (expected https or http)"
        )));
    }
    if rest.is_empty() {
        return Err(GitToolError::InvalidUrl("remote URL is empty".into()));
    }
    // Strip any pre-existing user-info segment so we don't leak
    // credentials or end up with `user:token@newuser:newtoken@host`.
    let without_auth = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    if without_auth.is_empty() {
        return Err(GitToolError::InvalidUrl("remote URL has no host".into()));
    }
    // Reject obvious control chars in the JWT. The JWT itself is
    // user-provided and ends up on the command line, so make sure
    // no newline / whitespace / shell meta sneaks in — these are not
    // meaningful in a JWT anyway.
    for c in jwt.chars() {
        if c.is_ascii_whitespace() || c == '@' || c == '#' || c.is_control() {
            return Err(GitToolError::InvalidUrl(format!(
                "auth token contains disallowed character: {c:?}"
            )));
        }
    }
    Ok(format!("{scheme}://x-token:{jwt}@{without_auth}"))
}

fn redact_url(url: &str) -> String {
    // Collapse any user-info portion to `***` so logs never leak a JWT.
    if let Some((scheme, rest)) = url.split_once("://") {
        if let Some((_, host)) = rest.rsplit_once('@') {
            return format!("{scheme}://***@{host}");
        }
        return format!("{scheme}://{rest}");
    }
    url.to_string()
}

// -----------------------------------------------------------------------
// Tool implementations
// -----------------------------------------------------------------------

fn workspace_timeout(ctx: &ToolContext) -> Duration {
    // Keep this comfortably above the kernel's default command_timeout
    // (10s) since `git push` over slow networks routinely takes longer.
    // Tools still respect the hard upper bound via `max_async_timeout_ms`.
    Duration::from_millis(ctx.config.max_async_timeout_ms.min(120_000))
}

/// Push-specific policy derived from the tool context.
///
/// Separate from [`workspace_timeout`] so slow `git push` calls can
/// use their own (longer, configurable) per-attempt budget and a
/// bounded retry loop without dragging the rest of the git tools up
/// with them. See [`PushPolicy::from_config`] for the knob.
fn push_policy_for(ctx: &ToolContext) -> PushPolicy {
    PushPolicy::from_config(&ctx.config)
}

fn str_arg<'a>(args: &'a serde_json::Value, name: &'static str) -> Result<&'a str, GitToolError> {
    args.get(name)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or(GitToolError::MissingArg(name))
}

fn opt_bool(args: &serde_json::Value, name: &str) -> bool {
    args.get(name)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// `git_commit` — stage all changes (`git add -A`) and create a commit.
pub struct GitCommitTool;

#[async_trait]
impl Tool for GitCommitTool {
    fn name(&self) -> &str {
        "git_commit"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_commit".into(),
            description: "Stage every change under the workspace root (`git add -A`) and \
                create a commit with the provided message. Returns the new commit SHA, or \
                reports when there were no changes to commit. Runs inside the kernel's \
                sandboxed workspace root only."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["message"],
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "Commit message (non-empty)."
                    }
                }
            }),
            cache_control: None,
            eager_input_streaming: None,
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let message = str_arg(&args, "message").map_err(ToolError::from)?;
        let workspace = ctx.sandbox.root().to_path_buf();
        let timeout = workspace_timeout(ctx);

        let agent_id = ctx
            .caller_agent_id
            .map_or_else(|| "unknown".to_string(), |id| id.to_string());
        info!(op = "git_commit", %agent_id, "git tool dispatched");

        match git_commit_impl(&workspace, message, timeout).await {
            Ok(Some(sha)) => Ok(ToolResult::success(
                "git_commit",
                serde_json::to_string(&serde_json::json!({
                    "sha": sha,
                    "committed": true,
                }))
                .unwrap_or_else(|_| format!("committed {sha}")),
            )),
            Ok(None) => Ok(ToolResult::success(
                "git_commit",
                serde_json::to_string(&serde_json::json!({
                    "sha": null,
                    "committed": false,
                    "reason": "no staged changes",
                }))
                .unwrap_or_else(|_| "no staged changes".to_string()),
            )),
            Err(e) => {
                warn!(error = %e, "git_commit failed");
                Err(e.into())
            }
        }
    }
}

/// `git_push` — push `HEAD` to `<remote_url>:<branch>` with inline JWT auth.
pub struct GitPushTool;

#[async_trait]
impl Tool for GitPushTool {
    fn name(&self) -> &str {
        "git_push"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_push".into(),
            description: "Push HEAD to the given remote URL + branch using a JWT-authenticated \
                URL (no on-disk remote registration). Runs inside the kernel's sandboxed \
                workspace root only."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["remote_url", "branch", "jwt"],
                "properties": {
                    "remote_url": { "type": "string" },
                    "branch": { "type": "string" },
                    "jwt": { "type": "string" },
                    "force": { "type": "boolean" }
                }
            }),
            cache_control: None,
            eager_input_streaming: None,
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let remote_url = str_arg(&args, "remote_url").map_err(ToolError::from)?;
        let branch = str_arg(&args, "branch").map_err(ToolError::from)?;
        let jwt = str_arg(&args, "jwt").map_err(ToolError::from)?;
        let force = opt_bool(&args, "force");
        let workspace = ctx.sandbox.root().to_path_buf();
        let policy = push_policy_for(ctx);

        let agent_id = ctx
            .caller_agent_id
            .map_or_else(|| "unknown".to_string(), |id| id.to_string());
        info!(
            op = "git_push",
            %agent_id,
            target_branch = branch,
            remote = %redact_url(remote_url),
            per_attempt_timeout_ms = duration_millis_u64(policy.per_attempt_timeout),
            attempts = policy.attempts,
            "git tool dispatched"
        );

        match git_push_impl(&workspace, remote_url, branch, jwt, force, policy).await {
            Ok(commits) => Ok(ToolResult::success(
                "git_push",
                serde_json::to_string(&serde_json::json!({
                    "pushed": true,
                    "commits": commits
                        .iter()
                        .map(|c| serde_json::json!({"sha": c.sha, "message": c.message}))
                        .collect::<Vec<_>>(),
                }))
                .unwrap_or_else(|_| format!("pushed {} commits", commits.len())),
            )),
            Err(e) => {
                warn!(error = %e, "git_push failed");
                Err(e.into())
            }
        }
    }
}

/// `git_commit_push` — transactional `git add -A`, `git commit`, `git push`.
pub struct GitCommitPushTool;

#[async_trait]
impl Tool for GitCommitPushTool {
    fn name(&self) -> &str {
        "git_commit_push"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_commit_push".into(),
            description: "Stage all changes, commit with the given message, and push the \
                resulting HEAD to <remote_url>:<branch>. Runs as a single kernel-mediated \
                tool so the dev-loop can treat the pair as an atomic unit."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["message", "remote_url", "branch", "jwt"],
                "properties": {
                    "message": { "type": "string" },
                    "remote_url": { "type": "string" },
                    "branch": { "type": "string" },
                    "jwt": { "type": "string" },
                    "force": { "type": "boolean" }
                }
            }),
            cache_control: None,
            eager_input_streaming: None,
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let message = str_arg(&args, "message").map_err(ToolError::from)?;
        let remote_url = str_arg(&args, "remote_url").map_err(ToolError::from)?;
        let branch = str_arg(&args, "branch").map_err(ToolError::from)?;
        let jwt = str_arg(&args, "jwt").map_err(ToolError::from)?;
        let force = opt_bool(&args, "force");
        let workspace = ctx.sandbox.root().to_path_buf();
        let commit_timeout = workspace_timeout(ctx);
        let push_policy = push_policy_for(ctx);

        let agent_id = ctx
            .caller_agent_id
            .map_or_else(|| "unknown".to_string(), |id| id.to_string());
        info!(
            op = "git_commit_push",
            %agent_id,
            target_branch = branch,
            remote = %redact_url(remote_url),
            commit_timeout_ms = duration_millis_u64(commit_timeout),
            push_per_attempt_timeout_ms = duration_millis_u64(push_policy.per_attempt_timeout),
            push_attempts = push_policy.attempts,
            "git tool dispatched"
        );

        match git_commit_push_impl(
            &workspace,
            message,
            remote_url,
            branch,
            jwt,
            force,
            commit_timeout,
            push_policy,
        )
        .await
        {
            Ok(outcome) => {
                let CommitPushOutcome {
                    commit_sha,
                    push_result,
                } = outcome;
                match push_result {
                    Ok(commits) => Ok(ToolResult::success(
                        "git_commit_push",
                        serde_json::to_string(&serde_json::json!({
                            "sha": commit_sha,
                            "committed": commit_sha.is_some(),
                            "pushed": true,
                            "commits": commits
                                .iter()
                                .map(|c| serde_json::json!({"sha": c.sha, "message": c.message}))
                                .collect::<Vec<_>>(),
                        }))
                        .unwrap_or_else(|_| {
                            format!(
                                "committed+pushed {}",
                                commit_sha.as_deref().unwrap_or("(none)")
                            )
                        }),
                    )),
                    Err(push_err) => {
                        // Commit landed locally; push did not. Surface
                        // this as a success-with-warning so callers
                        // (the dev-loop automaton) can preserve the
                        // commit SHA in their event stream instead of
                        // dropping the work. The tool-level result
                        // reports `pushed: false` + `push_error` so
                        // the agent can see what happened and the
                        // automaton dispatcher can emit both
                        // `GitCommitted` and `GitPushFailed`.
                        warn!(
                            error = %push_err,
                            commit_sha = ?commit_sha,
                            "git_commit_push: commit succeeded but push failed"
                        );
                        Ok(ToolResult::success(
                            "git_commit_push",
                            serde_json::to_string(&serde_json::json!({
                                "sha": commit_sha,
                                "committed": commit_sha.is_some(),
                                "pushed": false,
                                "push_error": push_err.to_string(),
                                "commits": Vec::<serde_json::Value>::new(),
                            }))
                            .unwrap_or_else(|_| {
                                format!(
                                    "committed {} but push failed: {push_err}",
                                    commit_sha.as_deref().unwrap_or("(none)")
                                )
                            }),
                        ))
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "git_commit_push failed before commit");
                Err(e.into())
            }
        }
    }
}

/// Git tools that operate purely on the local workspace and never
/// reach a remote. Safe to set `on` for any automaton
/// that already has write access to the workspace — the mutation is
/// contained by the `GitExecutor` workspace-escape checks.
pub const GIT_LOCAL_TOOL_NAMES: &[&str] = &["git_commit"];

/// Git tools that talk to a remote (require both a repo URL and an
/// auth token to be meaningful). Without remote configuration, these
/// should usually remain unavailable or ask-gated by the caller's
/// tri-state tool policy so a misconfigured workspace does not silently
/// push to the wrong upstream.
pub const GIT_REMOTE_TOOL_NAMES: &[&str] = &["git_push", "git_commit_push"];

/// The full set of git tool names registered by this module.
pub const GIT_TOOL_NAMES: &[&str] = &["git_commit", "git_push", "git_commit_push"];

#[cfg(test)]
mod tests;
