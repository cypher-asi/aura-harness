//! The PRs of a repo on github.com, for the REPO tab: listed, and one's
//! diff, through the `gh` CLI when it is installed and signed in, else
//! GitHub's REST API with `GH_TOKEN` or `GITHUB_TOKEN`.
//!
//! The token is read here and sent to api.github.com alone; it goes in no
//! log, view or error.

use async_trait::async_trait;
use serde::Deserialize;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::process::Command;

/// How long one `gh` call or API request may take.
const TIMEOUT: Duration = Duration::from_secs(20);

/// Most PRs listed: the most recently updated.
const MAX_PRS: usize = 30;

/// What the REPO tab says when neither `gh` nor a token can read PRs.
pub(super) const SIGN_IN: &str = "Sign in with gh auth login, or set GITHUB_TOKEN, to list PRs.";

/// `owner/repo` on github.com.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct Slug {
    pub owner: String,
    pub repo: String,
}

impl std::fmt::Display for Slug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.repo)
    }
}

/// The repo a remote's URL points at, when it is on github.com:
/// `https://github.com/o/r.git`, `git@github.com:o/r`,
/// `ssh://git@github.com/o/r`.
pub(super) fn slug(url: &str) -> Option<Slug> {
    let url = url.trim();
    let (host, path) = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?,
        None => url.split_once(':')?,
    };
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    let host = host.split_once(':').map_or(host, |(host, _)| host);
    if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com") {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    let fine = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    (fine(owner) && fine(repo) && repo != "." && repo != "..").then(|| Slug {
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

/// Where a PR stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PrStatus {
    Open,
    Draft,
    Merged,
    Closed,
}

/// A PR, as either source tells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Pr {
    pub number: u64,
    pub title: String,
    pub status: PrStatus,
    pub head: String,
    pub base: String,
    /// Opened, merged and closed at, in Unix seconds
    pub created: i64,
    pub merged: Option<i64>,
    pub closed: Option<i64>,
    pub url: String,
}

impl Pr {
    /// When it last changed: merged, else closed, else opened.
    pub(super) fn at(&self) -> i64 {
        self.merged.or(self.closed).unwrap_or(self.created)
    }
}

/// Why a source gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Failure {
    /// It can't be used here: `gh` is missing or signed out, or there is
    /// no token. The next source is tried.
    Unavailable,
    /// It was asked and failed; said as is.
    Failed(String),
}

/// A way to read a repo's PRs.
#[async_trait]
pub(super) trait Source: Send + Sync {
    async fn list(&self, slug: &Slug) -> Result<Vec<Pr>, Failure>;
    async fn diff(&self, slug: &Slug, number: u64) -> Result<String, Failure>;
}

/// `gh`, then the API.
pub(super) fn sources() -> Vec<Box<dyn Source>> {
    vec![Box::new(Gh), Box::new(Api::from_env())]
}

/// The PRs of `slug` from the first of `sources` that can read them.
pub(super) async fn list(sources: &[Box<dyn Source>], slug: &Slug) -> Result<Vec<Pr>, String> {
    for source in sources {
        match source.list(slug).await {
            Ok(mut prs) => {
                prs.truncate(MAX_PRS);
                return Ok(prs);
            }
            Err(Failure::Unavailable) => {}
            Err(Failure::Failed(e)) => return Err(e),
        }
    }
    Err(SIGN_IN.to_string())
}

/// The diff of PR `number` of `slug`, from the first of `sources` that
/// can read it.
pub(super) async fn diff(
    sources: &[Box<dyn Source>],
    slug: &Slug,
    number: u64,
) -> Result<String, String> {
    for source in sources {
        match source.diff(slug, number).await {
            Ok(diff) => return Ok(diff),
            Err(Failure::Unavailable) => {}
            Err(Failure::Failed(e)) => return Err(e),
        }
    }
    Err(SIGN_IN.to_string())
}

/// The `gh` CLI.
struct Gh;

/// What `gh pr list --json` gives for each PR.
const GH_FIELDS: &str =
    "number,title,state,isDraft,headRefName,baseRefName,createdAt,mergedAt,closedAt,url";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPr {
    number: u64,
    title: String,
    state: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    head_ref_name: String,
    #[serde(default)]
    base_ref_name: String,
    created_at: Option<String>,
    merged_at: Option<String>,
    closed_at: Option<String>,
    #[serde(default)]
    url: String,
}

#[async_trait]
impl Source for Gh {
    async fn list(&self, slug: &Slug) -> Result<Vec<Pr>, Failure> {
        let limit = MAX_PRS.to_string();
        let repo = slug.to_string();
        let out = gh(&[
            "pr", "list", "--repo", &repo, "--state", "all", "--limit", &limit, "--json", GH_FIELDS,
        ])
        .await?;
        gh_prs(&out).map_err(Failure::Failed)
    }

    async fn diff(&self, slug: &Slug, number: u64) -> Result<String, Failure> {
        let (number, repo) = (number.to_string(), slug.to_string());
        gh(&["pr", "diff", &number, "--repo", &repo, "--color", "never"]).await
    }
}

/// What `gh pr list --json` printed, as PRs.
fn gh_prs(out: &str) -> Result<Vec<Pr>, String> {
    let listed: Vec<GhPr> = serde_json::from_str(out)
        .map_err(|e| format!("gh pr list printed what it shouldn't: {e}"))?;
    Ok(listed
        .into_iter()
        .map(|pr| {
            let status = match pr.state.as_str() {
                "MERGED" => PrStatus::Merged,
                "CLOSED" => PrStatus::Closed,
                _ if pr.is_draft => PrStatus::Draft,
                _ => PrStatus::Open,
            };
            Pr {
                number: pr.number,
                title: pr.title,
                status,
                head: pr.head_ref_name,
                base: pr.base_ref_name,
                created: pr.created_at.as_deref().and_then(time).unwrap_or(0),
                merged: pr.merged_at.as_deref().and_then(time),
                closed: pr.closed_at.as_deref().and_then(time),
                url: pr.url,
            }
        })
        .collect())
}

/// `gh <args>`, prompting for nothing.
async fn gh(args: &[&str]) -> Result<String, Failure> {
    let mut command = Command::new("gh");
    command
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_SPINNER_DISABLED", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(TIMEOUT, command.output()).await {
        Err(_) => return Err(Failure::Failed("gh took too long".to_string())),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => return Err(Failure::Unavailable),
        Ok(Err(e)) => return Err(Failure::Failed(format!("gh: {e}"))),
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    // gh exits 4 when it needs signing in.
    if output.status.code() == Some(4) {
        return Err(Failure::Unavailable);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let said = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("failed");
    Err(Failure::Failed(format!("gh: {said}")))
}

/// GitHub's REST API, with a token.
struct Api {
    token: Option<String>,
}

#[derive(Deserialize)]
struct ApiPr {
    number: u64,
    title: String,
    state: String,
    #[serde(default)]
    draft: bool,
    head: ApiRef,
    base: ApiRef,
    created_at: Option<String>,
    merged_at: Option<String>,
    closed_at: Option<String>,
    #[serde(default)]
    html_url: String,
}

#[derive(Deserialize)]
struct ApiRef {
    #[serde(rename = "ref", default)]
    name: String,
}

impl Api {
    fn from_env() -> Self {
        let token = ["GH_TOKEN", "GITHUB_TOKEN"].into_iter().find_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|token| !token.trim().is_empty())
        });
        Self { token }
    }

    async fn get(&self, path: &str, accept: &str) -> Result<String, Failure> {
        let Some(token) = &self.token else {
            return Err(Failure::Unavailable);
        };
        let client = client().map_err(Failure::Failed)?;
        let response = client
            .get(format!("https://api.github.com{path}"))
            .bearer_auth(token)
            .header("Accept", accept)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| Failure::Failed(format!("GitHub: {}", e.without_url())))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| Failure::Failed(format!("GitHub: {}", e.without_url())))?;
        if status.is_success() {
            return Ok(body);
        }
        let said = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|body| body.get("message")?.as_str().map(str::to_string))
            .unwrap_or_else(|| status.to_string());
        Err(Failure::Failed(format!("GitHub: {said}")))
    }
}

#[async_trait]
impl Source for Api {
    async fn list(&self, slug: &Slug) -> Result<Vec<Pr>, Failure> {
        let path = format!(
            "/repos/{}/{}/pulls?state=all&sort=updated&direction=desc&per_page={MAX_PRS}",
            slug.owner, slug.repo
        );
        let body = self.get(&path, "application/vnd.github+json").await?;
        api_prs(&body).map_err(Failure::Failed)
    }

    async fn diff(&self, slug: &Slug, number: u64) -> Result<String, Failure> {
        let path = format!("/repos/{}/{}/pulls/{number}", slug.owner, slug.repo);
        self.get(&path, "application/vnd.github.diff").await
    }
}

/// The API's PR list, as PRs.
fn api_prs(body: &str) -> Result<Vec<Pr>, String> {
    let listed: Vec<ApiPr> =
        serde_json::from_str(body).map_err(|e| format!("GitHub sent what it shouldn't: {e}"))?;
    Ok(listed
        .into_iter()
        .map(|pr| {
            let merged = pr.merged_at.as_deref().and_then(time);
            let status = match (pr.state.as_str(), merged) {
                (_, Some(_)) => PrStatus::Merged,
                ("closed", None) => PrStatus::Closed,
                _ if pr.draft => PrStatus::Draft,
                _ => PrStatus::Open,
            };
            Pr {
                number: pr.number,
                title: pr.title,
                status,
                head: pr.head.name,
                base: pr.base.name,
                created: pr.created_at.as_deref().and_then(time).unwrap_or(0),
                merged,
                closed: pr.closed_at.as_deref().and_then(time),
                url: pr.html_url,
            }
        })
        .collect())
}

/// The client every API request shares.
fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(TIMEOUT)
                .user_agent(concat!("aura/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|e| format!("GitHub: {e}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// An RFC 3339 time in Unix seconds; `None` for gh's zero time.
fn time(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.timestamp())
        .filter(|at| *at > 0)
}

#[cfg(test)]
mod tests;
