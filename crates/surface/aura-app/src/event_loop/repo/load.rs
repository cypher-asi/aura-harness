//! The REPO tab's view of the repo an agent works in: its branch and
//! changes, a timeline of what happened in it, and its branches, tags
//! and worktrees.
//!
//! Commits come from the branch's first-parent history. What else moved
//! `HEAD` (merges that left no commit, rebases, pulls, resets, checkouts)
//! comes from its reflog, and pushes and fetches from the reflogs of the
//! remote-tracking branches, so none of it asks the network. PRs come
//! from GitHub, through [`super::prs`].

use super::github::{Pr, PrStatus};
use super::parse::{self, Commit, HeadMove, Moved, Ref, RemoteMove};
use super::{git, github_repo, keep, prs, CACHE};
use crate::event_loop::subagents::lock;
use aura_core_types::AgentId;
use aura_ui_terminal::repo::{
    EventKind, EventState, PrState, RepoBranch, RepoEvent, RepoTag, RepoTarget, RepoView,
    RepoWorktree,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio::task::JoinHandle;

/// Commits read along the branch.
const COMMITS: usize = 50;
/// `HEAD`'s reflog entries read.
const HEAD_MOVES: usize = 300;
/// Reflog entries read for each remote-tracking branch.
const REMOTE_MOVES: usize = 50;
/// Remote-tracking branches whose reflogs are read beside the upstream:
/// those with the newest commits.
const REMOTES: usize = 10;
/// Most tags listed.
const TAGS: usize = 50;
/// Most commit counts worked out per load; moves past them say none.
const COUNTS: usize = 40;
/// How far apart, in seconds, a pull's fetch and its move of `HEAD` may be.
const PULL_SLACK: i64 = 10;

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

    let status = parse::status(
        &git(
            &root,
            &[
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=normal",
            ],
        )
        .await?,
    );
    let born = status.oid.is_some();
    let refs = parse::refs(
        &git(
            &root,
            &[
                "for-each-ref",
                parse::REF_FORMAT,
                "refs/heads",
                "refs/remotes",
                "refs/tags",
                "refs/stash",
            ],
        )
        .await?,
    );
    let commits = if born {
        let limit = format!("-n{COMMITS}");
        parse::commits(
            &git(
                &root,
                &["log", "--first-parent", "-z", &limit, parse::COMMIT_FORMAT],
            )
            .await?,
        )
    } else {
        Vec::new()
    };
    let has_remotes = refs.iter().any(|r| r.name.starts_with("refs/remotes/"));
    let pending = pending(&root, &status, has_remotes).await;

    let mut events: Vec<RepoEvent> = commits
        .iter()
        .map(|commit| commit_event(commit, pending.contains(&commit.sha)))
        .collect();
    let mut budget = COUNTS;
    let head_moves = if born {
        head_moves(&root).await
    } else {
        Vec::new()
    };
    let merges: HashSet<&str> = commits
        .iter()
        .filter(|c| c.parents > 1)
        .map(|c| c.sha.as_str())
        .collect();
    let pulls: Vec<i64> = head_moves
        .iter()
        .filter(|m| m.what == HeadMove::Pull)
        .map(|m| m.at)
        .collect();
    for moved in head_moves {
        if matches!(moved.what, HeadMove::Merge { .. }) && merges.contains(moved.to.as_str()) {
            continue;
        }
        let count = count(&root, moved.from.as_deref(), &moved.to, &mut budget).await;
        events.push(head_event(&moved, count, status.upstream.as_deref()));
    }
    for (branch, moves) in remote_moves(&root, &refs, status.upstream.as_deref()).await {
        for moved in moves {
            let pulled = moved.what == RemoteMove::Pull
                && pulls.iter().any(|at| (at - moved.at).abs() <= PULL_SLACK);
            if pulled {
                continue;
            }
            let count = count(&root, moved.from.as_deref(), &moved.to, &mut budget).await;
            events.push(remote_event(&moved, &branch, count));
        }
    }
    if refs.iter().any(|r| r.name == "refs/stash") {
        let stashes = git(&root, &["stash", "list", "-z", parse::STASH_FORMAT])
            .await
            .unwrap_or_default();
        events.extend(parse::stashes(&stashes).into_iter().map(|stash| RepoEvent {
            kind: EventKind::Stash,
            state: EventState::Done,
            at: stash.at,
            title: stash.subject,
            detail: stash.name.clone(),
            target: RepoTarget::Stash { name: stash.name },
            url: None,
        }));
    }

    let remote = refs
        .iter()
        .find(|r| r.head && r.name.starts_with("refs/heads/"))
        .and_then(|r| r.remote.clone());
    let (prs, reading) = match github_repo(&root, remote.as_deref()).await {
        None => (PrState::NotGitHub, None),
        Some(slug) => {
            let (answer, reading) = prs(&slug, force).await;
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
    };
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

/// The commits of `HEAD` not pushed yet: those its upstream lacks, or
/// with no upstream, those on no remote. A repo with no remotes pushes
/// nowhere, so none of its commits wait.
async fn pending(root: &Path, status: &parse::Status, has_remotes: bool) -> HashSet<String> {
    let args: &[&str] = match &status.upstream {
        Some(_) if status.ahead == 0 => return HashSet::new(),
        Some(_) => &["rev-list", "-n1000", "@{upstream}..HEAD"],
        None if has_remotes && status.oid.is_some() => {
            &["rev-list", "-n1000", "HEAD", "--not", "--remotes"]
        }
        None => return HashSet::new(),
    };
    git(root, args)
        .await
        .map(|out| out.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn commit_event(commit: &Commit, pending: bool) -> RepoEvent {
    let mut detail = commit.short.clone();
    if !commit.refs.is_empty() {
        detail = format!("{detail} · {}", commit.refs.join(", "));
    }
    RepoEvent {
        kind: if commit.parents > 1 {
            EventKind::Merge
        } else {
            EventKind::Commit
        },
        state: if pending {
            EventState::Pending
        } else {
            EventState::Done
        },
        at: commit.at,
        title: commit.subject.clone(),
        detail,
        target: RepoTarget::Commit {
            sha: commit.sha.clone(),
        },
        url: None,
    }
}

async fn head_moves(root: &Path) -> Vec<Moved<HeadMove>> {
    let limit = format!("-n{}", HEAD_MOVES + 1);
    let out = git(
        root,
        &[
            "log",
            "-g",
            "-z",
            "--date=unix",
            &limit,
            parse::REFLOG_FORMAT,
            "HEAD",
        ],
    )
    .await
    .unwrap_or_default();
    parse::head_moves(&parse::reflog(&out))
}

/// The moves of the upstream and of the [`REMOTES`] remote-tracking
/// branches with the newest commits, each by its short name.
async fn remote_moves(
    root: &Path,
    refs: &[Ref],
    upstream: Option<&str>,
) -> Vec<(String, Vec<Moved<RemoteMove>>)> {
    let upstream = upstream.map(|up| format!("refs/remotes/{up}"));
    let mut remotes: Vec<&Ref> = refs
        .iter()
        .filter(|r| r.name.starts_with("refs/remotes/") && !r.name.ends_with("/HEAD"))
        .filter(|r| Some(&r.name) != upstream.as_ref())
        .collect();
    remotes.sort_by_key(|r| std::cmp::Reverse(r.at));
    remotes.truncate(REMOTES);
    if let Some(up) = refs.iter().find(|r| Some(&r.name) == upstream.as_ref()) {
        remotes.insert(0, up);
    }
    let mut found = Vec::new();
    for remote in remotes {
        let key = (root.to_path_buf(), remote.name.clone());
        let kept = lock(&CACHE.remotes)
            .get(&key)
            .filter(|(tip, _)| *tip == remote.sha)
            .map(|(_, moves)| moves.clone());
        let moves = match kept {
            Some(moves) => moves,
            None => {
                let limit = format!("-n{}", REMOTE_MOVES + 1);
                let out = git(
                    root,
                    &[
                        "log",
                        "-g",
                        "-z",
                        "--date=unix",
                        &limit,
                        parse::REFLOG_FORMAT,
                        &remote.name,
                    ],
                )
                .await
                .unwrap_or_default();
                let entries = parse::reflog(&out);
                let moves = parse::remote_moves(&entries, entries.len() <= REMOTE_MOVES);
                keep(
                    &mut lock(&CACHE.remotes),
                    key,
                    (remote.sha.clone(), moves.clone()),
                );
                moves
            }
        };
        let name = remote.name.trim_start_matches("refs/remotes/").to_string();
        found.push((name, moves));
    }
    found
}

/// The commits in `from..to`, while `budget` lasts.
async fn count(root: &Path, from: Option<&str>, to: &str, budget: &mut usize) -> Option<usize> {
    let from = from?;
    let key = (from.to_string(), to.to_string());
    if let Some(&n) = lock(&CACHE.counts).get(&key) {
        return Some(n);
    }
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    let range = format!("{from}..{to}");
    let n: usize = git(root, &["rev-list", "--count", &range])
        .await
        .ok()?
        .trim()
        .parse()
        .ok()?;
    keep(&mut lock(&CACHE.counts), key, n);
    Some(n)
}

/// `3 commits`, `1 commit`.
fn commits(n: usize) -> String {
    format!("{n} commit{}", if n == 1 { "" } else { "s" })
}

/// `a1b2c3d4..e5f6a7b8`, or the end alone.
fn range(from: Option<&str>, to: &str) -> String {
    match from {
        Some(from) => format!("{}..{}", short(from), short(to)),
        None => short(to).to_string(),
    }
}

/// A full sha cut short, other names as they are.
fn name_of(name: &str) -> &str {
    if name.len() == 40 && parse::is_sha(name) {
        short(name)
    } else {
        name
    }
}

fn head_event(moved: &Moved<HeadMove>, count: Option<usize>, upstream: Option<&str>) -> RepoEvent {
    let (kind, title) = match &moved.what {
        HeadMove::Merge { branch } => (EventKind::Merge, format!("Merged {branch}")),
        HeadMove::Rebase { branch, onto } => {
            let title = match (branch, onto) {
                (Some(branch), Some(onto)) => format!("Rebased {branch} onto {}", name_of(onto)),
                (None, Some(onto)) => format!("Rebased onto {}", name_of(onto)),
                (Some(branch), None) => format!("Rebased {branch}"),
                (None, None) => "Rebased".to_string(),
            };
            (EventKind::Rebase, title)
        }
        HeadMove::Pull => {
            let what = match count {
                Some(n) if n > 0 => format!("Pulled {}", commits(n)),
                _ => "Pulled".to_string(),
            };
            let title = match upstream {
                Some(upstream) => format!("{what} from {upstream}"),
                None => what,
            };
            (EventKind::Pull, title)
        }
        HeadMove::Reset { to } => (EventKind::Reset, format!("Reset to {}", name_of(to))),
        HeadMove::Checkout { from, to } => (
            EventKind::Checkout,
            format!("Switched from {} to {}", name_of(from), name_of(to)),
        ),
    };
    RepoEvent {
        kind,
        state: EventState::Done,
        at: moved.at,
        title,
        detail: range(moved.from.as_deref(), &moved.to),
        target: RepoTarget::Range {
            from: moved.from.clone(),
            to: moved.to.clone(),
        },
        url: None,
    }
}

fn remote_event(moved: &Moved<RemoteMove>, branch: &str, count: Option<usize>) -> RepoEvent {
    let push = moved.what == RemoteMove::Push;
    let title = match (moved.from.as_ref(), count, push) {
        (None, _, true) => format!("Pushed new branch {branch}"),
        (None, _, false) => format!("Fetched new branch {branch}"),
        (Some(_), Some(n), true) if n > 0 => format!("Pushed {} to {branch}", commits(n)),
        (Some(_), Some(n), false) if n > 0 => format!("Fetched {} to {branch}", commits(n)),
        (Some(_), _, true) => format!("Pushed to {branch}"),
        (Some(_), _, false) => format!("Fetched {branch}"),
    };
    RepoEvent {
        kind: if push {
            EventKind::Push
        } else {
            EventKind::Fetch
        },
        state: EventState::Done,
        at: moved.at,
        title,
        detail: range(moved.from.as_deref(), &moved.to),
        target: RepoTarget::Range {
            from: moved.from.clone(),
            to: moved.to.clone(),
        },
        url: None,
    }
}

fn pr_event(pr: &Pr) -> RepoEvent {
    RepoEvent {
        kind: EventKind::Pr,
        state: match pr.status {
            PrStatus::Open => EventState::Open,
            PrStatus::Draft => EventState::Draft,
            PrStatus::Merged => EventState::Merged,
            PrStatus::Closed => EventState::Closed,
        },
        at: pr.at(),
        title: format!("#{} {}", pr.number, pr.title),
        detail: format!("{} → {}", pr.head, pr.base),
        target: RepoTarget::Pr { number: pr.number },
        url: Some(pr.url.clone()).filter(|url| url.starts_with("https://github.com/")),
    }
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
