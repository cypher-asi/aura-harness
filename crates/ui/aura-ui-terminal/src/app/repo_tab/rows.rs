//! The rows of the REPO tab's list, what each is across refreshes, and
//! what opening one does.

use super::RepoFilter;
use crate::repo::{EventKind, PrState, RepoBranch, RepoEvent, RepoTarget, RepoView};
use std::path::PathBuf;

/// A row of the REPO tab's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoRow {
    /// The branch, its upstream and how far apart they are
    Summary,
    /// The header the uncommitted changes fold under
    Changes,
    /// An uncommitted change, by its index in the view's
    Change(usize),
    /// How many changes are past those listed
    MoreChanges,
    /// An event of the timeline, by its index in the view's
    Event(usize),
    Branch(usize),
    Tag(usize),
    Worktree(usize),
    /// Why no PRs are listed
    PrNote,
}

/// What a row is, kept across refreshes to keep the highlight on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RowKey {
    Summary,
    Changes,
    Change(String),
    MoreChanges,
    Event(EventKind, RepoTarget),
    Branch(String),
    Tag(String),
    Worktree(PathBuf),
    PrNote,
}

pub(super) fn key_of(view: &RepoView, row: RepoRow) -> Option<RowKey> {
    Some(match row {
        RepoRow::Summary => RowKey::Summary,
        RepoRow::Changes => RowKey::Changes,
        RepoRow::Change(i) => RowKey::Change(view.changes.get(i)?.path.clone()),
        RepoRow::MoreChanges => RowKey::MoreChanges,
        RepoRow::Event(i) => {
            let event = view.events.get(i)?;
            RowKey::Event(event.kind, event.target.clone())
        }
        RepoRow::Branch(i) => RowKey::Branch(view.branches.get(i)?.name.clone()),
        RepoRow::Tag(i) => RowKey::Tag(view.tags.get(i)?.name.clone()),
        RepoRow::Worktree(i) => RowKey::Worktree(view.worktrees.get(i)?.path.clone()),
        RepoRow::PrNote => RowKey::PrNote,
    })
}

/// The rows `view` lists under `filter`, the changes folded or not.
pub(super) fn rows_of(view: &RepoView, filter: RepoFilter, changes_folded: bool) -> Vec<RepoRow> {
    let mut rows = Vec::new();
    match filter {
        RepoFilter::Branches => rows.extend((0..view.branches.len()).map(RepoRow::Branch)),
        RepoFilter::Tags => rows.extend((0..view.tags.len()).map(RepoRow::Tag)),
        RepoFilter::Worktrees => rows.extend((0..view.worktrees.len()).map(RepoRow::Worktree)),
        _ => {
            rows.push(RepoRow::Summary);
            let changes = !view.changes.is_empty();
            if filter == RepoFilter::All && changes {
                rows.push(RepoRow::Changes);
            }
            let open = filter == RepoFilter::Changes || !changes_folded;
            if matches!(filter, RepoFilter::All | RepoFilter::Changes) && changes && open {
                rows.extend((0..view.changes.len()).map(RepoRow::Change));
                if view.changes_more > 0 {
                    rows.push(RepoRow::MoreChanges);
                }
            }
            if filter == RepoFilter::Prs && matches!(view.prs, PrState::Note(_)) {
                rows.push(RepoRow::PrNote);
            }
            let since = view.span_start.filter(|_| filter == RepoFilter::All);
            rows.extend(
                view.events
                    .iter()
                    .enumerate()
                    .filter(|(_, event)| filter.shows(event.kind))
                    .filter(|(_, event)| since.is_none_or(|start| event.at >= start))
                    .map(|(i, _)| RepoRow::Event(i)),
            );
        }
    }
    rows
}

/// What opening a row does.
pub(super) enum Opened {
    /// Post this diff in the chat, under this title
    Diff(RepoTarget, String),
    /// Fold the changes or open them
    Fold,
    /// Say why there is nothing to post
    Say(String),
    Nothing,
}

/// `3 commits`, `1 commit`.
fn commits(n: usize) -> String {
    format!("{n} commit{}", if n == 1 { "" } else { "s" })
}

pub(super) fn opened(view: &RepoView, row: RepoRow) -> Opened {
    match row {
        RepoRow::Summary => opened_summary(view),
        RepoRow::Changes => Opened::Fold,
        RepoRow::Change(i) => view.changes.get(i).map_or(Opened::Nothing, |change| {
            let target = RepoTarget::Change {
                path: change.path.clone(),
                from: change.from.clone(),
            };
            Opened::Diff(target, format!("Uncommitted: {}", change.path))
        }),
        RepoRow::Event(i) => view.events.get(i).map_or(Opened::Nothing, opened_event),
        RepoRow::Branch(i) => view.branches.get(i).map_or(Opened::Nothing, opened_branch),
        RepoRow::Tag(i) => view.tags.get(i).map_or(Opened::Nothing, |tag| {
            let target = RepoTarget::Tag {
                name: tag.name.clone(),
            };
            Opened::Diff(target, format!("Since {}", tag.name))
        }),
        RepoRow::Worktree(i) => view.worktrees.get(i).map_or(Opened::Nothing, |worktree| {
            let target = RepoTarget::Worktree {
                path: worktree.path.clone(),
            };
            Opened::Diff(
                target,
                format!("Uncommitted in {}", worktree.path.display()),
            )
        }),
        RepoRow::PrNote => match &view.prs {
            PrState::Note(note) => Opened::Say(note.clone()),
            _ => Opened::Nothing,
        },
        RepoRow::MoreChanges => Opened::Nothing,
    }
}

/// Opening the sync row: what the branch has that its upstream lacks.
fn opened_summary(view: &RepoView) -> Opened {
    let Some(branch) = &view.branch else {
        return Opened::Say(format!("HEAD is detached at {}", view.head));
    };
    let Some(upstream) = &view.upstream else {
        return Opened::Say(format!("{branch} has no upstream to compare with"));
    };
    if view.ahead == 0 {
        return Opened::Say(format!("Nothing to push: {upstream} has all of {branch}"));
    }
    let title = format!("{branch}: {} not on {upstream}", commits(view.ahead));
    Opened::Diff(RepoTarget::Unpushed, title)
}

/// Opening an event of the timeline, a commit titled by its short sha.
fn opened_event(event: &RepoEvent) -> Opened {
    let title = match &event.target {
        RepoTarget::Commit { sha } => {
            format!("{} {}", sha.get(..8).unwrap_or(sha), event.title)
        }
        _ => event.title.clone(),
    };
    Opened::Diff(event.target.clone(), title)
}

/// Opening a branch: what it has that HEAD lacks, unless it is HEAD's.
fn opened_branch(branch: &RepoBranch) -> Opened {
    if branch.current {
        return Opened::Say(format!("{} is checked out here", branch.name));
    }
    let target = RepoTarget::Branch {
        name: branch.name.clone(),
    };
    Opened::Diff(target, format!("What {} has that HEAD lacks", branch.name))
}
