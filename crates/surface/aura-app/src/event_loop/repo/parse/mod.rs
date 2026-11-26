//! What the REPO tab's git calls print, read into plain values. Each
//! parser reads one call [`super::load`] makes, with the format named
//! beside it. The reflogs' are in `reflogs`.

use aura_ui::repo::{ChangeKind, RepoChange};
use std::path::PathBuf;

mod reflogs;

#[cfg(test)]
pub(super) use reflogs::Entry;
pub(super) use reflogs::{
    head_moves, reflog, remote_moves, HeadMove, Moved, RemoteMove, REFLOG_FORMAT,
};

/// Most uncommitted changes listed; the rest are counted.
pub(super) const MAX_CHANGES: usize = 300;

/// `status --porcelain=v2 --branch -z`
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Status {
    /// `HEAD`'s sha; `None` before the first commit
    pub oid: Option<String>,
    /// Branch checked out; `None` when detached
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub changes: Vec<RepoChange>,
    /// Changes past [`MAX_CHANGES`]
    pub more: usize,
}

pub(super) fn status(out: &str) -> Status {
    let mut status = Status::default();
    let mut records = out.split('\0');
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("# ") {
            status.header(header);
            continue;
        }
        if let Some(change) = change(record, &mut records) {
            status.add(change);
        }
    }
    status
}

impl Status {
    /// Read a `# branch.*` record, `header` without its `# `.
    fn header(&mut self, header: &str) {
        let (key, value) = header.split_once(' ').unwrap_or((header, ""));
        match key {
            "branch.oid" => {
                self.oid = Some(value)
                    .filter(|v| *v != "(initial)")
                    .map(str::to_string);
            }
            "branch.head" => {
                self.branch = Some(value)
                    .filter(|v| *v != "(detached)")
                    .map(str::to_string);
            }
            "branch.upstream" => self.upstream = Some(value.to_string()),
            "branch.ab" => {
                let mut counts = value.split(' ');
                let mut count = |sign: char| {
                    counts
                        .next()
                        .and_then(|count| count.strip_prefix(sign))
                        .and_then(|count| count.parse().ok())
                        .unwrap_or(0)
                };
                self.ahead = count('+');
                self.behind = count('-');
            }
            _ => {}
        }
    }

    /// List `change`, or count it once [`MAX_CHANGES`] are listed.
    fn add(&mut self, change: RepoChange) {
        if self.changes.len() < MAX_CHANGES {
            self.changes.push(change);
        } else {
            self.more += 1;
        }
    }
}

/// The change a record that isn't a header lists, if it lists one. A
/// rename's (`2`) path before it is the record after it, read from
/// `records`.
fn change<'a>(record: &str, records: &mut impl Iterator<Item = &'a str>) -> Option<RepoChange> {
    match record.split_once(' ') {
        Some(("1", rest)) => rest
            .splitn(8, ' ')
            .collect::<Vec<_>>()
            .split_first()
            .and_then(|(xy, fields)| Some(tracked(xy, fields.get(6)?, None))),
        Some(("2", rest)) => {
            let from = records.next().map(str::to_string);
            rest.splitn(9, ' ')
                .collect::<Vec<_>>()
                .split_first()
                .and_then(|(xy, fields)| Some(tracked(xy, fields.get(7)?, from)))
        }
        Some(("u", rest)) => rest.splitn(10, ' ').nth(9).map(|path| RepoChange {
            path: path.to_string(),
            from: None,
            kind: ChangeKind::Conflicted,
            staged: false,
            unstaged: true,
        }),
        Some(("?", path)) => Some(RepoChange {
            path: path.to_string(),
            from: None,
            kind: ChangeKind::Untracked,
            staged: false,
            unstaged: true,
        }),
        _ => None,
    }
}

/// A `1` or `2` record's change; `xy` is its staged and unstaged letters.
fn tracked(xy: &str, path: &str, from: Option<String>) -> RepoChange {
    let mut letters = xy.chars();
    let (x, y) = (letters.next().unwrap_or('.'), letters.next().unwrap_or('.'));
    let either = |letter: char| x == letter || y == letter;
    let kind = if x == 'A' {
        ChangeKind::Added
    } else if from.is_some() {
        ChangeKind::Renamed
    } else if either('D') {
        ChangeKind::Deleted
    } else if either('T') {
        ChangeKind::TypeChanged
    } else {
        ChangeKind::Modified
    };
    RepoChange {
        path: path.to_string(),
        from,
        kind,
        staged: x != '.',
        unstaged: y != '.',
    }
}

/// The format of the commits [`commits`] reads, with `log -z`.
pub(super) const COMMIT_FORMAT: &str = "--format=%H%x1f%h%x1f%P%x1f%ct%x1f%D%x1f%s";

/// A commit `log` listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Commit {
    pub sha: String,
    pub short: String,
    pub parents: usize,
    /// Committed at, in Unix seconds
    pub at: i64,
    /// Branches and tags at it, without `HEAD`
    pub refs: Vec<String>,
    pub subject: String,
}

pub(super) fn commits(out: &str) -> Vec<Commit> {
    out.split('\0')
        .filter_map(|record| {
            let mut fields = record.trim_matches('\n').split('\x1f');
            let sha = fields.next().filter(|sha| !sha.is_empty())?.to_string();
            let short = fields.next()?.to_string();
            let parents = fields.next()?.split_whitespace().count();
            let at = fields.next()?.parse().ok()?;
            let refs = fields
                .next()?
                .split(", ")
                .filter_map(|name| {
                    let name = name.strip_prefix("HEAD -> ").unwrap_or(name);
                    let shown = name.strip_prefix("tag: ").unwrap_or(name);
                    (!shown.is_empty() && shown != "HEAD" && !shown.ends_with("/HEAD"))
                        .then(|| shown.to_string())
                })
                .collect();
            let subject = fields.next().unwrap_or_default().to_string();
            Some(Commit {
                sha,
                short,
                parents,
                at,
                refs,
                subject,
            })
        })
        .collect()
}

/// The format of the refs [`refs`] reads, with `for-each-ref`.
pub(super) const REF_FORMAT: &str = "--format=%(refname)%1f%(objectname)%1f%(objectname:short)\
%1f%(upstream:short)%1f%(upstream:track,nobracket)%1f%(upstream:remotename)%1f%(creatordate:unix)\
%1f%(HEAD)%1f%(contents:subject)";

/// A branch, remote-tracking branch, tag or the stash.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Ref {
    /// In full: `refs/heads/main`
    pub name: String,
    pub sha: String,
    pub short: String,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    /// Whether its upstream was deleted
    pub gone: bool,
    /// The remote its upstream is on
    pub remote: Option<String>,
    /// When it (a tag) or its commit was made, in Unix seconds
    pub at: i64,
    /// Whether it is checked out here
    pub head: bool,
    pub subject: String,
}

pub(super) fn refs(out: &str) -> Vec<Ref> {
    out.lines()
        .filter_map(|line| {
            let mut fields = line.split('\x1f');
            let name = fields.next().filter(|name| !name.is_empty())?.to_string();
            let sha = fields.next()?.to_string();
            let short = fields.next()?.to_string();
            let upstream = fields
                .next()
                .filter(|up| !up.is_empty())
                .map(str::to_string);
            let track = fields.next()?;
            let remote = fields.next().filter(|r| !r.is_empty()).map(str::to_string);
            let at = fields.next()?.parse().unwrap_or(0);
            let head = fields.next()? == "*";
            let subject = fields.next().unwrap_or_default().to_string();
            let (ahead, behind) =
                track.split(", ").fold((0, 0), |(ahead, behind), part| {
                    match part.split_once(' ') {
                        Some(("ahead", n)) => (n.parse().unwrap_or(0), behind),
                        Some(("behind", n)) => (ahead, n.parse().unwrap_or(0)),
                        _ => (ahead, behind),
                    }
                });
            Some(Ref {
                name,
                sha,
                short,
                upstream,
                ahead,
                behind,
                gone: track == "gone",
                remote,
                at,
                head,
                subject,
            })
        })
        .collect()
}

/// The format of the stashes [`stashes`] reads, with `stash list -z`.
pub(super) const STASH_FORMAT: &str = "--format=%gd%x1f%ct%x1f%gs";

/// A stash: its name (`stash@{0}`), when it was made, and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Stash {
    pub name: String,
    pub at: i64,
    pub subject: String,
}

pub(super) fn stashes(out: &str) -> Vec<Stash> {
    out.split('\0')
        .filter_map(|record| {
            let mut fields = record.trim_matches('\n').split('\x1f');
            let name = fields
                .next()
                .filter(|name| name.starts_with("stash@{"))?
                .to_string();
            let at = fields.next()?.parse().ok()?;
            let subject = fields.next().unwrap_or_default().to_string();
            Some(Stash { name, at, subject })
        })
        .collect()
}

/// A worktree `worktree list --porcelain` listed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Worktree {
    pub path: PathBuf,
    pub head: String,
    /// Branch checked out, without `refs/heads/`
    pub branch: Option<String>,
    pub bare: bool,
    pub locked: bool,
    pub prunable: bool,
}

pub(super) fn worktrees(out: &str) -> Vec<Worktree> {
    let mut found: Vec<Worktree> = Vec::new();
    for line in out.lines() {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        if key == "worktree" {
            found.push(Worktree {
                path: PathBuf::from(value),
                ..Worktree::default()
            });
            continue;
        }
        let Some(worktree) = found.last_mut() else {
            continue;
        };
        match key {
            "HEAD" => worktree.head = value.to_string(),
            "branch" => {
                worktree.branch = Some(
                    value
                        .strip_prefix("refs/heads/")
                        .unwrap_or(value)
                        .to_string(),
                );
            }
            "bare" => worktree.bare = true,
            "locked" => worktree.locked = true,
            "prunable" => worktree.prunable = true,
            _ => {}
        }
    }
    found
}

/// Whether `text` could be a commit's sha, in full or cut short.
pub(super) fn is_sha(text: &str) -> bool {
    (4..=64).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether `name` names a stash: `stash@{3}`.
pub(super) fn is_stash(name: &str) -> bool {
    name.strip_prefix("stash@{")
        .and_then(|n| n.strip_suffix('}'))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}
