//! The REPO tab's timeline ([`super::load`]): the branch's commits, the
//! moves of `HEAD` and of the remote-tracking branches their reflogs
//! keep, the stashes and the PRs, each as an event.

use super::github::{Pr, PrStatus};
use super::load::short;
use super::parse::{self, Commit, HeadMove, Moved, Ref, RemoteMove, Status};
use super::{git, keep, CACHE};
use crate::event_loop::subagents::lock;
use aura_ui::repo::{EventKind, EventState, RepoEvent, RepoTarget};
use std::collections::HashSet;
use std::path::Path;

/// `HEAD`'s reflog entries read.
const HEAD_MOVES: usize = 300;
/// Reflog entries read for each remote-tracking branch.
const REMOTE_MOVES: usize = 50;
/// Remote-tracking branches whose reflogs are read beside the upstream:
/// those with the newest commits.
const REMOTES: usize = 10;
/// Most commit counts worked out per load; moves past them say none.
const COUNTS: usize = 40;
/// How far apart, in seconds, a pull's fetch and its move of `HEAD` may be.
const PULL_SLACK: i64 = 10;

/// What happened in the repo at `root`, whose `status` and `refs` were
/// read, in no order: its `commits`, then the moves of `HEAD` and of the
/// remote-tracking branches, then its stashes.
pub(super) async fn timeline(
    root: &Path,
    status: &Status,
    refs: &[Ref],
    commits: &[Commit],
) -> Vec<RepoEvent> {
    let has_remotes = refs.iter().any(|r| r.name.starts_with("refs/remotes/"));
    let pending = pending(root, status, has_remotes).await;
    let mut events: Vec<RepoEvent> = commits
        .iter()
        .map(|commit| commit_event(commit, pending.contains(&commit.sha)))
        .collect();
    let mut budget = COUNTS;
    let head_moves = if status.oid.is_some() {
        head_moves(root).await
    } else {
        Vec::new()
    };
    let pulls: Vec<i64> = head_moves
        .iter()
        .filter(|m| m.what == HeadMove::Pull)
        .map(|m| m.at)
        .collect();
    let upstream = status.upstream.as_deref();
    events.extend(head_events(root, &head_moves, commits, upstream, &mut budget).await);
    events.extend(remote_events(root, refs, upstream, &pulls, &mut budget).await);
    if refs.iter().any(|r| r.name == "refs/stash") {
        events.extend(stash_events(root).await);
    }
    events
}

/// The commits of `HEAD` not pushed yet: those its upstream lacks, or
/// with no upstream, those on no remote. A repo with no remotes pushes
/// nowhere, so none of its commits wait.
async fn pending(root: &Path, status: &Status, has_remotes: bool) -> HashSet<String> {
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

/// The events of `HEAD`'s `moves` but the merges its `commits` show, the
/// commits of each counted while `budget` lasts.
async fn head_events(
    root: &Path,
    moves: &[Moved<HeadMove>],
    commits: &[Commit],
    upstream: Option<&str>,
    budget: &mut usize,
) -> Vec<RepoEvent> {
    let merges: HashSet<&str> = commits
        .iter()
        .filter(|c| c.parents > 1)
        .map(|c| c.sha.as_str())
        .collect();
    let mut events = Vec::new();
    for moved in moves {
        if matches!(moved.what, HeadMove::Merge { .. }) && merges.contains(moved.to.as_str()) {
            continue;
        }
        let count = count(root, moved.from.as_deref(), &moved.to, budget).await;
        events.push(head_event(moved, count, upstream));
    }
    events
}

/// The events of the remote-tracking branches' moves ([`remote_moves`])
/// but the fetches of the `pulls` of `HEAD`, by when they were, the
/// commits of each counted while `budget` lasts.
async fn remote_events(
    root: &Path,
    refs: &[Ref],
    upstream: Option<&str>,
    pulls: &[i64],
    budget: &mut usize,
) -> Vec<RepoEvent> {
    let mut events = Vec::new();
    for (branch, moves) in remote_moves(root, refs, upstream).await {
        for moved in moves {
            let pulled = moved.what == RemoteMove::Pull
                && pulls.iter().any(|at| (at - moved.at).abs() <= PULL_SLACK);
            if pulled {
                continue;
            }
            let count = count(root, moved.from.as_deref(), &moved.to, budget).await;
            events.push(remote_event(&moved, &branch, count));
        }
    }
    events
}

/// The moves of the upstream and of the [`REMOTES`] remote-tracking
/// branches with the newest commits, each by its short name.
async fn remote_moves(
    root: &Path,
    refs: &[Ref],
    upstream: Option<&str>,
) -> Vec<(String, Vec<Moved<RemoteMove>>)> {
    let mut found = Vec::new();
    for remote in remotes_read(refs, upstream) {
        let moves = moves_of(root, remote).await;
        let name = remote.name.trim_start_matches("refs/remotes/").to_string();
        found.push((name, moves));
    }
    found
}

/// The remote-tracking branches of `refs` whose moves are read: the
/// `upstream`, then the [`REMOTES`] others with the newest commits.
fn remotes_read<'a>(refs: &'a [Ref], upstream: Option<&str>) -> Vec<&'a Ref> {
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
    remotes
}

/// The moves of the remote-tracking branch `remote`, from its reflog, or
/// as last read when its tip hasn't moved since.
async fn moves_of(root: &Path, remote: &Ref) -> Vec<Moved<RemoteMove>> {
    let key = (root.to_path_buf(), remote.name.clone());
    let kept = lock(&CACHE.remotes)
        .get(&key)
        .filter(|(tip, _)| *tip == remote.sha)
        .map(|(_, moves)| moves.clone());
    if let Some(moves) = kept {
        return moves;
    }
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

/// The repo's stashes, as events.
async fn stash_events(root: &Path) -> Vec<RepoEvent> {
    let stashes = git(root, &["stash", "list", "-z", parse::STASH_FORMAT])
        .await
        .unwrap_or_default();
    parse::stashes(&stashes)
        .into_iter()
        .map(|stash| RepoEvent {
            kind: EventKind::Stash,
            state: EventState::Done,
            at: stash.at,
            title: stash.subject,
            detail: stash.name.clone(),
            target: RepoTarget::Stash { name: stash.name },
            url: None,
        })
        .collect()
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

pub(super) fn pr_event(pr: &Pr) -> RepoEvent {
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
