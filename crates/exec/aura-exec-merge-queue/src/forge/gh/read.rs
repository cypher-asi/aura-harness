//! Reading what `gh` printed: a PR, a list of PRs, a new PR's number, how
//! a merge went, and which errors hold the queue.

use super::super::{ForgeError, MergeOutcome, PrState, PullRequest};
use crate::report::PrRef;
use serde::Deserialize;

/// How a merge of a PR whose head was to be `head` went, from the PR as
/// it is after the call, and what the call said when it failed.
pub(crate) fn merge_outcome(
    now: &PullRequest,
    head: &str,
    refusal: Option<String>,
) -> MergeOutcome {
    match (now.state, &now.merge_commit) {
        (PrState::Merged, Some(sha)) => MergeOutcome::Merged { sha: sha.clone() },
        (PrState::Merged, None) => {
            MergeOutcome::Refused("merged, but GitHub names no merge commit".to_string())
        }
        _ if now.head != head => MergeOutcome::HeadMoved {
            now: now.head.clone(),
        },
        (PrState::Closed, _) => {
            MergeOutcome::Refused("the PR was closed without merging".to_string())
        }
        (PrState::Open, _) => MergeOutcome::Refused(
            refusal.unwrap_or_else(|| "GitHub did not merge it and gave no reason".to_string()),
        ),
    }
}

/// A failed call's error: [`ForgeError::Unavailable`] when GitHub can't
/// be reached or the credentials are refused, else
/// [`ForgeError::Failed`].
pub(crate) fn classify(what: &str, stderr: &str) -> ForgeError {
    // Credentials GitHub refuses fail the entry with a message that says
    // so: waiting would never fix them.
    const DENIED: &[&str] = &[
        "gh auth login",
        "not logged in",
        "authentication",
        "bad credentials",
        "http 401",
        "http 403",
        "resource not accessible",
        "must have admin rights",
        "permission",
    ];
    const UNAVAILABLE: &[&str] = &[
        "could not resolve host",
        "connection refused",
        "connection reset",
        "i/o timeout",
        "tls handshake",
        "rate limit",
        "http 502",
        "http 503",
        "http 504",
    ];
    let lower = stderr.to_ascii_lowercase();
    let said = format!("{what}: {stderr}");
    if DENIED.iter().any(|marker| lower.contains(marker)) {
        return ForgeError::Failed(format!(
            "GitHub refused the merge queue's credentials (401/403): sign the hub's gh in \
             (gh auth login), or store a token with repo access in the Integrations panel. {said}"
        ));
    }
    if UNAVAILABLE.iter().any(|marker| lower.contains(marker)) {
        ForgeError::Unavailable(said)
    } else {
        ForgeError::Failed(said)
    }
}

/// What `gh pr view/list --json` gives for a PR.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPr {
    number: u64,
    url: String,
    state: String,
    head_ref_oid: String,
    #[serde(default)]
    merge_commit: Option<GhCommit>,
}

#[derive(Deserialize)]
struct GhCommit {
    oid: String,
}

impl From<GhPr> for PullRequest {
    fn from(pr: GhPr) -> Self {
        let state = match pr.state.as_str() {
            "MERGED" => PrState::Merged,
            "CLOSED" => PrState::Closed,
            _ => PrState::Open,
        };
        Self {
            pr: PrRef {
                number: pr.number,
                url: pr.url,
            },
            state,
            head: pr.head_ref_oid,
            merge_commit: pr.merge_commit.map(|commit| commit.oid),
        }
    }
}

pub(crate) fn parse_pr(json: &str) -> Result<PullRequest, ForgeError> {
    serde_json::from_str::<GhPr>(json)
        .map(PullRequest::from)
        .map_err(|e| ForgeError::Failed(format!("gh printed a PR it shouldn't: {e}")))
}

pub(crate) fn parse_prs(json: &str) -> Result<Vec<PullRequest>, ForgeError> {
    serde_json::from_str::<Vec<GhPr>>(json)
        .map(|prs| prs.into_iter().map(PullRequest::from).collect())
        .map_err(|e| ForgeError::Failed(format!("gh printed PRs it shouldn't: {e}")))
}

/// The PR number at the end of its address (`.../pull/12`).
pub(crate) fn number_in_url(url: &str) -> Option<u64> {
    let line = url.lines().rev().find(|line| line.contains("/pull/"))?;
    line.trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()?
        .parse()
        .ok()
}
