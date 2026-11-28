//! [`GhForge`]: GitHub through its `gh` CLI, which the hub has signed in
//! (or a token the hub passes as `GH_TOKEN`).
//!
//! Every call names the repo (`--repo owner/name`), so the CLI never
//! reads or changes a local checkout, and has a time limit. The token
//! goes to `gh` alone; it is in no log or error.
//!
//! ## What `gh` writes on GitHub (declared in `docs/invariants.md`)
//!
//! - `gh pr create`: opens a branch's PR when it has none.
//! - `gh pr comment`: says on a PR why it didn't merge, or which tests
//!   were flaky.
//! - `gh pr close`: closes a PR whose head is on the target already.
//! - `gh api --method POST .../statuses/<sha>`: the `aura/merge-queue`
//!   commit status.
//! - `gh pr merge` ([`Forge::merge`]) exists for the trait, but the
//!   landing path doesn't call it: the queue pushes its tested merge
//!   commit to the target itself, so the target is only ever a tested
//!   commit.
//!
//! It reads with `gh pr list` and `gh pr view`. The subcommands are
//! pinned by a check in `scripts/check_invariants.sh`. Branches are
//! never written through `gh`: pushes and deletions go through the
//! queue's own git ([`crate::git`]).

use super::{
    CheckState, Forge, ForgeError, MergeOutcome, PrRequest, PullRequest, FORGE_TIMEOUT,
    STATUS_CONTEXT,
};
use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// The fields every PR read asks for.
const PR_FIELDS: &str = "number,url,state,headRefOid,mergeCommit";
/// Longest commit status description GitHub takes.
const STATUS_TEXT_CHARS: usize = 140;

/// `owner/name` of a repo on github.com.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSlug {
    /// The repo's owner
    pub owner: String,
    /// The repo's name
    pub name: String,
}

impl RepoSlug {
    /// The repo a remote's URL points at, when it is on github.com:
    /// `https://github.com/o/r.git`, `git@github.com:o/r`,
    /// `ssh://git@github.com/o/r`. Credentials in the URL are ignored.
    #[must_use]
    pub fn from_remote_url(url: &str) -> Option<Self> {
        let url = url.trim();
        let (host, path) = match url.split_once("://") {
            Some((_, rest)) => rest.split_once('/')?,
            None => url.split_once(':')?,
        };
        let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
        let host = host.split_once(':').map_or(host, |(host, _)| host);
        if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com")
        {
            return None;
        }
        let path = path.trim_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        let (owner, name) = path.split_once('/')?;
        let fine = |part: &str| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        };
        (fine(owner) && fine(name)).then(|| Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }
}

impl std::fmt::Display for RepoSlug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// Where a [`GhForge`] gets a token on each call; `None` leaves `gh` to
/// its environment and its own sign-in.
pub type TokenSource = std::sync::Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// GitHub through the `gh` CLI.
#[derive(Clone)]
pub struct GhForge {
    slug: RepoSlug,
    token: Option<String>,
    /// Asked for a token on each call when `token` is unset: a token the
    /// hub stores, which may change while the queue runs
    token_source: Option<TokenSource>,
    /// The `gh` program, named `gh` so `scripts/check_invariants.sh`
    /// sees where it runs
    gh: PathBuf,
    timeout: Duration,
}

impl std::fmt::Debug for GhForge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GhForge")
            .field("slug", &self.slug)
            .field("token", &self.token.as_ref().map(|_| "<set>"))
            .finish_non_exhaustive()
    }
}

/// The GitHub repo `origin` of the repo at `top` points at; `None` when
/// it has no `origin` or it is not on github.com. Runs `git remote
/// get-url` through the queue's own git ([`crate::git`]).
pub async fn origin_slug(top: &std::path::Path) -> Option<RepoSlug> {
    let timeout = std::time::Duration::from_secs(15);
    let url = crate::git::ok(top, &["remote", "get-url", "origin"], timeout)
        .await
        .ok()?;
    RepoSlug::from_remote_url(&url)
}

impl GhForge {
    /// The forge of repo `slug`, through `gh` on the `PATH` and its own
    /// sign-in.
    #[must_use]
    pub fn new(slug: RepoSlug) -> Self {
        Self {
            slug,
            token: None,
            token_source: None,
            gh: PathBuf::from("gh"),
            timeout: FORGE_TIMEOUT,
        }
    }

    /// This forge, authenticating with `token` instead of `gh`'s sign-in.
    #[must_use]
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token.filter(|token| !token.trim().is_empty());
        self
    }

    /// This forge, asking `source` for a token on each call it makes
    /// without a fixed one ([`Self::with_token`] wins).
    #[must_use]
    pub fn with_token_source(mut self, source: TokenSource) -> Self {
        self.token_source = Some(source);
        self
    }

    /// This forge, running `program` as `gh` (tests).
    #[must_use]
    pub fn with_program(mut self, program: PathBuf) -> Self {
        self.gh = program;
        self
    }

    /// The repo it works on.
    #[must_use]
    pub const fn slug(&self) -> &RepoSlug {
        &self.slug
    }

    /// Run `gh <args>` with `stdin`; its standard output.
    async fn gh(&self, args: &[&str], stdin: Option<&str>) -> Result<String, ForgeError> {
        let mut command = Command::new(&self.gh);
        command
            .args(args)
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("NO_COLOR", "1")
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let asked = || self.token_source.as_ref().and_then(|source| source());
        let token = self.token.clone().or_else(asked);
        if let Some(token) = token.filter(|token| !token.trim().is_empty()) {
            command.env("GH_TOKEN", token);
        }
        let what = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
        let run = async {
            let mut child = command.spawn().map_err(|e| {
                ForgeError::Unavailable(format!("could not run {}: {e}", self.gh.display()))
            })?;
            if let (Some(text), Some(mut input)) = (stdin, child.stdin.take()) {
                input.write_all(text.as_bytes()).await.map_err(|e| {
                    ForgeError::Failed(format!("gh {what}: writing its input: {e}"))
                })?;
            }
            child
                .wait_with_output()
                .await
                .map_err(|e| ForgeError::Failed(format!("gh {what}: {e}")))
        };
        let output = tokio::time::timeout(self.timeout, run)
            .await
            .map_err(|_| {
                ForgeError::Unavailable(format!(
                    "gh {what} timed out after {}s",
                    self.timeout.as_secs()
                ))
            })??;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if output.status.success() {
            return Ok(stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(classify(&format!("gh {what}"), &stderr))
    }

    async fn list_open(&self, request: &PrRequest<'_>) -> Result<Option<PullRequest>, ForgeError> {
        let repo = self.slug.to_string();
        let listed = self
            .gh(
                &[
                    "pr",
                    "list",
                    "--repo",
                    &repo,
                    "--head",
                    request.branch,
                    "--base",
                    request.target,
                    "--state",
                    "open",
                    "--limit",
                    "1",
                    "--json",
                    PR_FIELDS,
                ],
                None,
            )
            .await?;
        Ok(parse_prs(&listed)?.into_iter().next())
    }
}

#[async_trait]
impl Forge for GhForge {
    async fn open_or_find(&self, request: &PrRequest<'_>) -> Result<PullRequest, ForgeError> {
        if let Some(open) = self.list_open(request).await? {
            return Ok(open);
        }
        let repo = self.slug.to_string();
        let created = self
            .gh(
                &[
                    "pr",
                    "create",
                    "--repo",
                    &repo,
                    "--base",
                    request.target,
                    "--head",
                    request.branch,
                    "--title",
                    request.title,
                    "--body-file",
                    "-",
                ],
                Some(request.body),
            )
            .await;
        match created {
            Ok(url) => match number_in_url(&url) {
                Some(number) => self.pr(number).await,
                None => Err(ForgeError::Failed(format!(
                    "gh pr create printed no PR address: {url}"
                ))),
            },
            // Opened by someone else in the meantime.
            Err(ForgeError::Failed(why)) if why.contains("already exists") => self
                .list_open(request)
                .await?
                .ok_or(ForgeError::Failed(why)),
            Err(error) => Err(error),
        }
    }

    async fn pr(&self, number: u64) -> Result<PullRequest, ForgeError> {
        let repo = self.slug.to_string();
        let shown = self
            .gh(
                &[
                    "pr",
                    "view",
                    &number.to_string(),
                    "--repo",
                    &repo,
                    "--json",
                    PR_FIELDS,
                ],
                None,
            )
            .await?;
        parse_pr(&shown)
    }

    async fn comment(&self, number: u64, body: &str) -> Result<(), ForgeError> {
        let repo = self.slug.to_string();
        self.gh(
            &[
                "pr",
                "comment",
                &number.to_string(),
                "--repo",
                &repo,
                "--body-file",
                "-",
            ],
            Some(body),
        )
        .await
        .map(drop)
    }

    async fn close(&self, number: u64, body: &str) -> Result<(), ForgeError> {
        let repo = self.slug.to_string();
        self.gh(
            &[
                "pr",
                "close",
                &number.to_string(),
                "--repo",
                &repo,
                "--comment",
                body,
            ],
            None,
        )
        .await
        .map(drop)
    }

    async fn set_status(&self, sha: &str, state: CheckState, text: &str) -> Result<(), ForgeError> {
        let path = format!("repos/{}/statuses/{sha}", self.slug);
        let state = format!("state={}", state.as_str());
        let context = format!("context={STATUS_CONTEXT}");
        let text: String = text.chars().take(STATUS_TEXT_CHARS).collect();
        let description = format!("description={text}");
        self.gh(
            &[
                "api",
                "--method",
                "POST",
                &path,
                "-f",
                &state,
                "-f",
                &context,
                "-f",
                &description,
            ],
            None,
        )
        .await
        .map(drop)
    }

    async fn merge(&self, number: u64, head: &str) -> Result<MergeOutcome, ForgeError> {
        let repo = self.slug.to_string();
        let merged = self
            .gh(
                &[
                    "pr",
                    "merge",
                    &number.to_string(),
                    "--repo",
                    &repo,
                    "--merge",
                    "--match-head-commit",
                    head,
                ],
                None,
            )
            .await;
        let refusal = match merged {
            Ok(_) => None,
            Err(ForgeError::Failed(why)) => Some(why),
            Err(unavailable) => return Err(unavailable),
        };
        let now = self.pr(number).await?;
        Ok(merge_outcome(&now, head, refusal))
    }
}

mod read;

pub(crate) use read::{classify, merge_outcome, number_in_url, parse_pr, parse_prs};

#[cfg(test)]
mod tests;
