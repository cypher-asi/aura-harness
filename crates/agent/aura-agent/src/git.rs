//! Read-only git helpers.
//!
//! Phase 2 remediation (Invariant §1 — Sole External Gateway):
//! **mutating** git operations (`git add`, `git commit`, `git push`,
//! `git remote set-url`) now live exclusively in
//! [`aura_exec_tools::git_tool`](../../../aura-exec-tools/src/git_tool/mod.rs),
//! where they run inside the kernel-mediated tool pipeline. This module
//! retains only the read-only helpers used by the agent surface
//! (`is_git_repo`, `list_unpushed_commits`, `read_only`) — the first is a
//! filesystem check, the others read the repo via `git` and are
//! explicitly listed as declared exceptions in `docs/invariants.md`.
//!
//! If you find yourself adding a mutating helper here, route it through
//! `aura_exec_tools::git_tool` instead.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::debug;

/// Timeout used for the read-only helpers below. Mutating operations
/// have their own timeout managed by the git tools in `aura-exec-tools`.
const GIT_READ_TIMEOUT: Duration = Duration::from_secs(aura_config::GIT_READ_TIMEOUT_SECS);

/// Lightweight record of a single commit surfaced by
/// [`list_unpushed_commits`]. Kept stable so external consumers
/// (aura-os run-log forwarder) that key off the commits payload do
/// not churn when the push helper is moved to `aura-exec-tools`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
}

/// Quick workspace-level check: does the directory contain a `.git`
/// entry? This is a pure filesystem probe — no subprocess, no kernel
/// mediation required.
#[must_use]
pub fn is_git_repo(project_root: &str) -> bool {
    Path::new(project_root).join(".git").exists()
}

async fn git_output(cmd: &mut Command, label: &str) -> Result<std::process::Output, String> {
    tokio::time::timeout(GIT_READ_TIMEOUT, cmd.output())
        .await
        .map_err(|_| {
            format!(
                "git {label}: timed out after {}s",
                GIT_READ_TIMEOUT.as_secs()
            )
        })?
        .map_err(|e| format!("git {label}: {e}"))
}

/// List commits on `HEAD` that have not yet been pushed to
/// `<remote>/<branch>`.
///
/// This is a **read-only** inspection used by dev-loop and push
/// telemetry. It does not mutate any repository state. When the
/// remote ref is missing (e.g. first push) we return an empty vector
/// rather than propagating an error.
pub async fn list_unpushed_commits(
    project_root: &str,
    remote: &str,
    branch: &str,
) -> Vec<CommitInfo> {
    let range = format!("{remote}/{branch}..HEAD");
    let output = git_output(
        Command::new("git")
            .args(["log", &range, "--pretty=format:%H %s"])
            .current_dir(project_root),
        "log (unpushed)",
    )
    .await;

    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|line| {
                let (sha, msg) = line.split_once(' ')?;
                Some(CommitInfo {
                    sha: sha.to_string(),
                    message: msg.to_string(),
                })
            })
            .collect(),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            debug!(%remote, %branch, %stderr, "no unpushed commits or ref not found");
            Vec::new()
        }
        Err(e) => {
            tracing::warn!(%remote, %branch, error = %e, "failed to list unpushed commits");
            Vec::new()
        }
    }
}

/// The subcommands [`read_only`] runs, by their leading words. None of
/// them writes the repo, its index, refs or config, or reaches the network.
const READ_ONLY: &[&[&str]] = &[
    &["rev-parse"],
    &["status"],
    &["log"],
    &["rev-list"],
    &["diff"],
    &["show"],
    &["ls-files"],
    &["for-each-ref"],
    &["merge-base"],
    &["stash", "list"],
    &["stash", "show"],
    &["worktree", "list"],
    &["remote", "get-url"],
];

/// The subcommands of [`READ_ONLY`] that can print a diff.
const DIFFS: &[&[&str]] = &[&["diff"], &["show"], &["log"], &["stash", "show"]];

/// Run `git <args>` in `dir` and return what it printed, when `args` is
/// one of the read-only subcommands in [`READ_ONLY`].
///
/// The repo is the agent's to change, `.git/config` included, so the
/// config can't make the call run anything: fsmonitor hooks and signature
/// checks are off, and a diff runs no external diff or textconv program
/// and names its files `a/` and `b/`, whatever the config says.
/// Nothing prompts, `status` takes no lock, paths are never patterns, the
/// repo is `dir`'s whatever `GIT_DIR` says, and messages are in English.
/// A `diff --no-index` that found differences exits 1, which is a success.
///
/// # Errors
///
/// When `args` is not read-only, or git fails: the first line of what
/// git said, without its `fatal: `.
pub async fn read_only(dir: &Path, args: &[&str]) -> Result<String, String> {
    let words = read_only_words(args)?;
    let (subcommand, rest) = args.split_at(words);
    let mut command = Command::new("git");
    command
        .args(["-c", "core.fsmonitor=false"])
        .args(["-c", "log.showSignature=false"])
        .args(["-c", "core.quotePath=false"])
        .args(subcommand);
    if DIFFS.contains(&subcommand) {
        command
            .args(["--no-ext-diff", "--no-textconv", "--no-color"])
            .args(["--src-prefix=a/", "--dst-prefix=b/"]);
    }
    command
        .args(rest)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let label = subcommand.join(" ");
    let output = git_output(&mut command, &label).await?;
    let no_index = subcommand == ["diff"] && rest.contains(&"--no-index");
    if output.status.success() || (no_index && output.status.code() == Some(1)) {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map_or_else(
            || format!("git {label} failed"),
            |line| line.trim_start_matches("fatal: ").to_string(),
        ))
}

/// How many of `args`' leading words name its read-only subcommand.
fn read_only_words(args: &[&str]) -> Result<usize, String> {
    let options = args.iter().take_while(|arg| **arg != "--");
    if options.clone().any(|arg| arg.starts_with("--output")) {
        return Err("git: --output would write a file".to_string());
    }
    READ_ONLY
        .iter()
        .find(|words| args.starts_with(words))
        .map(|words| words.len())
        .ok_or_else(|| {
            format!(
                "git {}: not a read-only subcommand",
                args.iter().take(2).copied().collect::<Vec<_>>().join(" ")
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git runs");
        assert!(status.status.success(), "git {args:?}: {status:?}");
    }

    #[test]
    fn only_read_only_subcommands_run() {
        for args in [
            &["status", "--porcelain"][..],
            &["log", "-n", "1"],
            &["stash", "list"],
            &["stash", "show", "-p", "stash@{0}"],
            &["worktree", "list", "--porcelain"],
            &["remote", "get-url", "origin"],
            &["diff", "--", "--output=notes.txt"],
        ] {
            assert!(read_only_words(args).is_ok(), "{args:?}");
        }
        for args in [
            &["commit", "-m", "x"][..],
            &["push"],
            &["fetch"],
            &["stash"],
            &["stash", "pop"],
            &["worktree", "add", "x"],
            &["remote", "add", "x", "y"],
            &["diff", "--output=notes.txt"],
            &["log", "--output", "notes.txt"],
            &[],
        ] {
            assert!(read_only_words(args).is_err(), "{args:?}");
        }
    }

    #[tokio::test]
    async fn reads_a_repo_without_running_its_config() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.email", "a@example.com"]);
        git(root, &["config", "user.name", "A"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(root, &["add", "a.txt"]);
        git(root, &["commit", "-q", "-m", "First"]);
        // An external diff that leaves a mark when it runs.
        let mark = root.join("ran");
        git(
            root,
            &[
                "config",
                "diff.external",
                &format!("touch {}", mark.display()),
            ],
        );
        std::fs::write(root.join("a.txt"), "two\n").unwrap();

        let diff = read_only(root, &["diff", "HEAD"]).await.unwrap();
        assert!(diff.contains("-one\n+two"), "{diff}");
        assert!(!mark.exists(), "the external diff ran");
        let log = read_only(root, &["log", "--format=%s"]).await.unwrap();
        assert_eq!(log.trim(), "First");

        let new = root.join("b.txt");
        std::fs::write(&new, "new\n").unwrap();
        let added = read_only(root, &["diff", "--no-index", "--", "/dev/null", "b.txt"])
            .await
            .unwrap();
        assert!(added.contains("+new"), "{added}");

        let error = read_only(root, &["log", "no-such-branch"])
            .await
            .unwrap_err();
        assert!(error.contains("no-such-branch"), "{error}");
        let refused = read_only(root, &["commit", "-m", "x"]).await.unwrap_err();
        assert!(refused.contains("not a read-only subcommand"), "{refused}");
    }
}
