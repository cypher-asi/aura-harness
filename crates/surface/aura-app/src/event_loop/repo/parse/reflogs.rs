//! What the reflogs say: their entries, and how `HEAD` and the
//! remote-tracking branches moved.

/// The format of the entries [`reflog`] reads, with `log -g -z
/// --date=unix`.
pub(in crate::event_loop::repo) const REFLOG_FORMAT: &str = "--format=%H%x1f%gD%x1f%gs";

/// One reflog entry: where a ref moved, when, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::event_loop::repo) struct Entry {
    pub sha: String,
    /// In Unix seconds
    pub at: i64,
    pub message: String,
}

pub(in crate::event_loop::repo) fn reflog(out: &str) -> Vec<Entry> {
    out.split('\0')
        .filter_map(|record| {
            let mut fields = record.trim_matches('\n').split('\x1f');
            let sha = fields.next().filter(|sha| !sha.is_empty())?.to_string();
            let selector = fields.next()?;
            let at = selector
                .rsplit_once("@{")
                .and_then(|(_, at)| at.strip_suffix('}'))
                .and_then(|at| at.parse().ok())?;
            let message = fields.next().unwrap_or_default().to_string();
            Some(Entry { sha, at, message })
        })
        .collect()
}

/// How `HEAD` moved, from its reflog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::event_loop::repo) enum HeadMove {
    /// `git merge <branch>`
    Merge { branch: String },
    /// A rebase of `branch` onto `onto`, from its start to its finish
    Rebase {
        branch: Option<String>,
        onto: Option<String>,
    },
    /// `git pull`
    Pull,
    /// `git reset` to `to`, as typed
    Reset { to: String },
    /// `git checkout` or `git switch`
    Checkout { from: String, to: String },
}

/// One move of `HEAD`, from `from` (`None` when the reflog was cut
/// before it) to `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::event_loop::repo) struct Moved<T> {
    pub what: T,
    pub at: i64,
    pub from: Option<String>,
    pub to: String,
}

/// The moves of `HEAD` in `entries`, its reflog newest first, that its
/// commits don't show: merges, rebases, pulls, resets and checkouts.
/// Each entry's old value is the next one's sha, so the oldest entry
/// only gives the one before it its start.
pub(in crate::event_loop::repo) fn head_moves(entries: &[Entry]) -> Vec<Moved<HeadMove>> {
    (0..entries.len())
        .filter_map(|i| head_move(entries, i))
        .collect()
}

/// The move `entries[i]` made, when [`head_moves`] lists it.
fn head_move(entries: &[Entry], i: usize) -> Option<Moved<HeadMove>> {
    let entry = entries.get(i)?;
    let before = entries.get(i + 1)?;
    let (action, said) = entry.message.split_once(": ")?;
    let from = Some(before.sha.clone());
    let (what, from) = if let Some(branch) = action.strip_prefix("merge ") {
        let what = HeadMove::Merge {
            branch: branch.to_string(),
        };
        (what, from)
    } else if action == "pull" || action.starts_with("pull ") {
        pull(entries, i, action, from)?
    } else if action.starts_with("rebase") {
        rebase(entries, i, action, said)?
    } else if action == "reset" {
        (reset(entry, before, said)?, from)
    } else if action == "checkout" {
        (checkout(said)?, from)
    } else {
        return None;
    };
    Some(Moved {
        what,
        at: entry.at,
        from,
        to: entry.sha.clone(),
    })
}

/// A pull at `entries[i]`, whose `action` names its step, if any, with
/// where it moved `HEAD` from: `from`, the entry before, for a pull of
/// one step, else where the pull started for its finish. `None` for its
/// other steps.
fn pull(
    entries: &[Entry],
    i: usize,
    action: &str,
    from: Option<String>,
) -> Option<(HeadMove, Option<String>)> {
    match step(action) {
        None => Some((HeadMove::Pull, from)),
        Some("finish") => {
            let from = started(entries, i, "pull").map(|(_, before)| before);
            Some((HeadMove::Pull, from))
        }
        Some(_) => None,
    }
}

/// A rebase finishing at `entries[i]`, which `said` how, with where it
/// started from; `None` for its other steps, which `action` names.
fn rebase(
    entries: &[Entry],
    i: usize,
    action: &str,
    said: &str,
) -> Option<(HeadMove, Option<String>)> {
    if step(action) != Some("finish") && action != "rebase finished" {
        return None;
    }
    let start = started(entries, i, "rebase");
    let from = start.as_ref().map(|(_, before)| before.clone());
    let what = HeadMove::Rebase {
        branch: said
            .strip_prefix("returning to ")
            .map(|to| to.strip_prefix("refs/heads/").unwrap_or(to).to_string()),
        onto: start.and_then(|(said, _)| said.strip_prefix("checkout ").map(str::to_string)),
    };
    Some((what, from))
}

/// A reset `entry` made, which `said` where to; `None` when it left
/// `HEAD` where it was `before`.
fn reset(entry: &Entry, before: &Entry, said: &str) -> Option<HeadMove> {
    if before.sha == entry.sha {
        return None;
    }
    Some(HeadMove::Reset {
        to: said.strip_prefix("moving to ").unwrap_or(said).to_string(),
    })
}

/// A checkout, from what `said` it moved from to what it moved to.
fn checkout(said: &str) -> Option<HeadMove> {
    let (from, to) = said
        .strip_prefix("moving from ")
        .and_then(|moved| moved.split_once(" to "))?;
    Some(HeadMove::Checkout {
        from: from.to_string(),
        to: to.to_string(),
    })
}

/// The step of a rebase an action names: `finish` in `rebase (finish)`.
fn step(action: &str) -> Option<&str> {
    action
        .strip_suffix(')')?
        .rsplit_once(" (")
        .map(|(_, step)| step)
}

/// Where the `family` sequence (`rebase`, `pull`) that finishes at
/// `entries[finish]` started: what its start said, and the sha `HEAD`
/// was at before it.
fn started(entries: &[Entry], finish: usize, family: &str) -> Option<(String, String)> {
    for (i, entry) in entries.iter().enumerate().skip(finish + 1) {
        let (action, said) = entry.message.split_once(": ")?;
        if !action.starts_with(family) {
            return None;
        }
        if step(action) == Some("start") {
            return Some((said.to_string(), entries.get(i + 1)?.sha.clone()));
        }
    }
    None
}

/// How a remote-tracking branch moved, from its reflog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::event_loop::repo) enum RemoteMove {
    /// `git push` moved it
    Push,
    /// `git fetch` moved it
    Fetch,
    /// The fetch of a `git pull`
    Pull,
}

/// The pushes and fetches in `entries`, a remote-tracking branch's reflog
/// newest first. When the reflog is `complete` its oldest entry made the
/// branch, from nothing.
pub(in crate::event_loop::repo) fn remote_moves(
    entries: &[Entry],
    complete: bool,
) -> Vec<Moved<RemoteMove>> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(i, entry)| {
            let from = match entries.get(i + 1) {
                Some(before) => Some(before.sha.clone()),
                None if complete => None,
                None => return None,
            };
            let action = entry.message.split(": ").next().unwrap_or_default();
            let what = if entry.message == "update by push" {
                RemoteMove::Push
            } else if action == "fetch" || action.starts_with("fetch ") {
                RemoteMove::Fetch
            } else if action == "pull" || action.starts_with("pull ") {
                RemoteMove::Pull
            } else {
                return None;
            };
            Some(Moved {
                what,
                at: entry.at,
                from,
                to: entry.sha.clone(),
            })
        })
        .collect()
}
