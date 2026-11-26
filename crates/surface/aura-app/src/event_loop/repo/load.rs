//! The REPO tab's view of the repo an agent works in: its branch and
//! changes, a timeline of what happened in it, and its branches, tags
//! and worktrees.
//!
//! Commits come from the branch's first-parent history. What else moved
//! `HEAD` (merges that left no commit, rebases, pulls, resets, checkouts)
//! comes from its reflog, and pushes and fetches from the reflogs of the
//! remote-tracking branches, so none of it asks the network
//! ([`super::timeline`]). PRs come from GitHub, through [`super::prs`].

use super::github;
use super::parse::{self, Commit, Ref, Status};
use super::timeline::{pr_event, timeline};
use super::{git, github_repo, prs};
use aura_core_types::AgentId;
use aura_ui::repo::{PrState, RepoBranch, RepoEvent, RepoTag, RepoView, RepoWorktree};
use std::path::{Path, PathBuf};
use tokio::task::JoinHandle;

/// Commits read along the branch.
const COMMITS: usize = 50;
/// Most tags listed.
const TAGS: usize = 50;

/// Said while GitHub is first asked for the PRs.
pub(super) const READING_PRS: &str = "Reading PRs…";

/// The REPO tab of `agent_id`, whose folder is `folder`; `force` asks
/// GitHub for the PRs again. With it comes the job reading PRs this load
/// asked for, when there is one: once it ends, a new load shows them.
pub(super) async fn view(
    folder: &Path,
    agent_id: AgentId,
    force: bool,
) -> (RepoView, Option<JoinHandle<()>>) {
    let agent_id = agent_id.to_hex();
    match read(folder, force).await {
        Ok((view, reading)) => (RepoView { agent_id, ..view }, reading),
        Err(problem) => (
            RepoView {
                agent_id,
                root: folder.to_path_buf(),
                problem: Some(problem),
                ..RepoView::default()
            },
            None,
        ),
    }
}

/// `problem`, said as the tab says it when `folder` is in no repo.
fn not_a_repo(problem: String) -> String {
    if problem
        .to_ascii_lowercase()
        .contains("not a git repository")
    {
        "Not a git repository.".to_string()
    } else {
        problem
    }
}

async fn read(folder: &Path, force: bool) -> Result<(RepoView, Option<JoinHandle<()>>), String> {
    let (root, worktree) = toplevel(folder).await?;
    let status = read_status(&root).await?;
    let refs = read_refs(&root).await?;
    let commits = read_commits(&root, status.oid.is_some()).await?;
    let mut events = timeline(&root, &status, &refs, &commits).await;
    let (prs, reading) = read_prs(&root, &refs, force, &mut events).await;
    events.sort_by_key(|event| std::cmp::Reverse(event.at));

    let view = RepoView {
        agent_id: String::new(),
        branch: status.branch.clone(),
        head: status
            .oid
            .as_deref()
            .map(short)
            .unwrap_or_default()
            .to_string(),
        upstream: status.upstream.clone(),
        ahead: status.ahead,
        behind: status.behind,
        worktree,
        changes: status.changes,
        changes_more: status.more,
        events,
        span_start: (commits.len() == COMMITS)
            .then(|| commits.last().map(|c| c.at))
            .flatten(),
        branches: branches(&refs),
        tags: tags(&refs),
        worktrees: worktrees(&root).await,
        prs,
        problem: None,
        root,
    };
    Ok((view, reading))
}

/// The root of the repo `folder` is in, and whether `folder` is in a
/// worktree of it that isn't its main one.
async fn toplevel(folder: &Path) -> Result<(PathBuf, bool), String> {
    let top = git(
        folder,
        &[
            "rev-parse",
            "--show-toplevel",
            "--absolute-git-dir",
            "--git-common-dir",
        ],
    )
    .await
    .map_err(not_a_repo)?;
    let mut lines = top.lines();
    let (Some(root), Some(git_dir), Some(common)) = (lines.next(), lines.next(), lines.next())
    else {
        return Err("git rev-parse said too little".to_string());
    };
    let root = PathBuf::from(root);
    let worktree = !same_dir(Path::new(git_dir), &folder.join(common));
    Ok((root, worktree))
}

async fn read_status(root: &Path) -> Result<Status, String> {
    let out = git(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=normal",
        ],
    )
    .await?;
    Ok(parse::status(&out))
}

async fn read_refs(root: &Path) -> Result<Vec<Ref>, String> {
    let out = git(
        root,
        &[
            "for-each-ref",
            parse::REF_FORMAT,
            "refs/heads",
            "refs/remotes",
            "refs/tags",
            "refs/stash",
        ],
    )
    .await?;
    Ok(parse::refs(&out))
}

/// The newest [`COMMITS`] along the branch; none before the first commit
/// (`born`).
async fn read_commits(root: &Path, born: bool) -> Result<Vec<Commit>, String> {
    if !born {
        return Ok(Vec::new());
    }
    let limit = format!("-n{COMMITS}");
    let out = git(
        root,
        &["log", "--first-parent", "-z", &limit, parse::COMMIT_FORMAT],
    )
    .await?;
    Ok(parse::commits(&out))
}

/// Where the PRs of the repo at `root` stand, with the events of those
/// listed added to `events`, and the job reading them when this load
/// asked GitHub ([`prs`]); `force` asks at once.
async fn read_prs(
    root: &Path,
    refs: &[Ref],
    force: bool,
    events: &mut Vec<RepoEvent>,
) -> (PrState, Option<JoinHandle<()>>) {
    let remote = refs
        .iter()
        .find(|r| r.head && r.name.starts_with("refs/heads/"))
        .and_then(|r| r.remote.clone());
    let Some(slug) = github_repo(root, remote.as_deref()).await else {
        return (PrState::NotGitHub, None);
    };
    let (answer, reading) = prs(&slug, force, github::sources).await;
    let state = match answer {
        None => PrState::Note(READING_PRS.to_string()),
        Some(Err(problem)) => PrState::Note(problem),
        Some(Ok(listed)) => {
            events.extend(listed.iter().map(pr_event));
            PrState::Listed
        }
    };
    (state, reading)
}

/// Whether `a` and `b` are the same folder.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// `sha` cut to the length the tab shows.
pub(super) fn short(sha: &str) -> &str {
    sha.get(..8).unwrap_or(sha)
}

fn branches(refs: &[Ref]) -> Vec<RepoBranch> {
    let mut branches: Vec<RepoBranch> = refs
        .iter()
        .filter_map(|r| {
            Some(RepoBranch {
                name: r.name.strip_prefix("refs/heads/")?.to_string(),
                current: r.head,
                upstream: r.upstream.clone(),
                ahead: r.ahead,
                behind: r.behind,
                gone: r.gone,
                at: r.at,
                subject: r.subject.clone(),
            })
        })
        .collect();
    branches.sort_by_key(|b| std::cmp::Reverse(b.at));
    branches
}

fn tags(refs: &[Ref]) -> Vec<RepoTag> {
    let mut tags: Vec<RepoTag> = refs
        .iter()
        .filter_map(|r| {
            Some(RepoTag {
                name: r.name.strip_prefix("refs/tags/")?.to_string(),
                subject: r.subject.clone(),
                at: r.at,
            })
        })
        .collect();
    tags.sort_by_key(|t| std::cmp::Reverse(t.at));
    tags.truncate(TAGS);
    tags
}

async fn worktrees(root: &Path) -> Vec<RepoWorktree> {
    let out = git(root, &["worktree", "list", "--porcelain"])
        .await
        .unwrap_or_default();
    let mut found: Vec<RepoWorktree> = parse::worktrees(&out)
        .into_iter()
        .filter(|w| !w.bare)
        .map(|w| RepoWorktree {
            current: same_dir(&w.path, root),
            head: short(&w.head).to_string(),
            path: w.path,
            branch: w.branch,
            locked: w.locked,
            prunable: w.prunable,
        })
        .collect();
    found.sort_by(|a, b| b.current.cmp(&a.current).then_with(|| a.path.cmp(&b.path)));
    found
}
